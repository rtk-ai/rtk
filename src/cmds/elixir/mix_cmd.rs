//! `mix test` and `mix precommit`: ExUnit output with the noise taken out and every failure kept.
//!
//! `mix compile` and `mix format` are TOML filters (`src/filters/mix-*.toml`). These two need
//! state a TOML filter can't keep: which compile section belongs to a dependency, where a
//! failure's stacktrace starts and ends, and whether a failure repeats an earlier one.
//! `mix precommit` is the alias `mix phx.new` generates (compile, format, credo, test, ...),
//! so its output is the same mix of compile and ExUnit output.
//!
//! What goes:
//! - dependency compile sections (`==> dep`, rebar3's `===> Compiling dep`), their warnings
//!   included, since the project can't act on a dependency's warnings. A section that
//!   reports a failure is kept whole.
//! - compile progress (`Compiling 12 files (.ex)`, `Generated my_app app`) and progress dots.
//! - top-level debug, info and notice log lines; a warning or error log line that repeats
//!   is shown once, with a count.
//! - in a stacktrace, each run of dependency frames, folded into one line naming the
//!   dependencies. A failure with an earlier failure's error and stacktrace (apart from its
//!   own test file's lines) points at that failure, keeping its `code:` line and the line
//!   in its test where it failed.
//!
//! Everything else stays as printed: each failure's header, location, error and the
//! project's own frames, credo and sobelow reports, the summary. What is cut can be read
//! back with `rtk recall`; without a recall store only the progress noise goes.

use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;

use crate::core::tracking;
use crate::core::utils::{ChildArgExt, resolved_command, strip_ansi};

/// The mix tasks filtered here. Every other task keeps its TOML filter or runs raw.
const TASKS: &[&str] = &["test", "precommit"];

static SECTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^==> (\S+)$").unwrap());
static REBAR_COMPILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^===> Compiling (\S+)$").unwrap());
static PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(Compiling \d+ files? \(\.\w+\)|Compiling \S+ \(it's taking more than \d+s\)|Generated \S+ app|===> Analyzing applications\.\.\.|[.*]+)$",
    )
    .unwrap()
});
/// A dependency section reporting this kept whole: the agent needs to see why it broke.
static DEP_FAILURE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)could not compile|compilation failed|^\s*\*\* \(|^\s*error[:\s]").unwrap()
});
/// A Logger line at the start of a line, after any progress dots ExUnit printed before it:
/// `12:00:00.000 [info] msg`, `2026-10-25T01:18:56.092004Z [error] msg`, with optional
/// `key=value` metadata before the level.
static LOG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[.*]*((?:\d{2}:\d{2}:\d{2}\.\d{3}|\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?) (?:\S+=\S* )*\[(\w+)\] ?(.*))$",
    )
    .unwrap()
});
/// ExUnit right-aligns the counter: `  1) test ...`, ` 10) test ...`, `100) test ...`.
static FAILURE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^ {0,3}(\d+)\) \S").unwrap());
static LOCATION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s+\S+:\d+$").unwrap());
/// `(phoenix 1.8.15) lib/phoenix/router.ex:428: Phoenix.Router.__call__/5`
static APP_FRAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s+)\((\w+) [^)]*\) \S").unwrap());
/// `test/my_app/foo_test.exs:13: (test)`: a frame with no app prefix.
static PATH_FRAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+\S+\.(?:exs?|erl):\d+: ").unwrap());
/// A frame in a test file: `test/my_app/foo_test.exs:13: (test)`.
static TEST_FRAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+(\S+_test\.exs:\d+): ").unwrap());
static APP_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*app:\s*:(\w+)").unwrap());
static APPS_PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"apps_path:\s*"([^"]+)""#).unwrap());

/// Runs `mix test` or `mix precommit` from rtk's fallback, which sees the raw argv
/// (`["mix", "test", ...]`). `None` for every other command.
pub fn try_run(args: &[String]) -> Option<Result<i32>> {
    handles(args).then(|| run(args))
}

fn handles(args: &[String]) -> bool {
    let is_mix = args
        .first()
        .and_then(|program| Path::new(program).file_name())
        .is_some_and(|name| name == "mix");
    is_mix
        && args
            .get(1)
            .is_some_and(|task| TASKS.contains(&task.as_str()))
}

fn run(args: &[String]) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let label = crate::core::shell::display_command(args);
    let (raw, exit_code) = run_merged(args)?;
    let clean = strip_ansi(&raw);
    let apps = project_apps(Path::new("."));

    let full = filter_output(&clean, &apps, true);
    let hint = full
        .cut
        .then(|| crate::core::tee::force_tee_hint(&clean, &label))
        .flatten();
    // Never cut what can't be read back: without a store only the progress noise goes.
    let filtered = if full.cut && hint.is_none() {
        filter_output(&clean, &apps, false).text
    } else {
        full.text
    };
    let shown = crate::core::runner::emit_guarded(&filtered, hint.as_deref(), &clean);
    timer.track(&label, &format!("rtk {label}"), &raw, &shown);
    Ok(exit_code)
}

/// Runs mix with stdout and stderr on one pipe, so a compile warning (stderr) stays inside
/// the section it belongs to (stdout) instead of trailing after the test results.
fn run_merged(args: &[String]) -> Result<(String, i32)> {
    let (mut reader, writer) = std::io::pipe().context("Failed to create a pipe for mix")?;
    let mut cmd = resolved_command(&args[0]);
    cmd.child_args(&args[1..])
        .stdin(Stdio::inherit())
        .stdout(writer.try_clone().context("Failed to share the pipe")?)
        .stderr(writer);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to run {}", args[0]))?;
    // The Command still holds both write ends, and the read only ends once they are closed.
    drop(cmd);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .context("Failed to read mix output")?;
    let status = child.wait().context("Failed to wait for mix")?;
    Ok((
        crate::core::utils::decode_process_output(&bytes),
        crate::core::utils::exit_code_from_status(&status, &args[0]),
    ))
}

/// The project's own OTP apps, from `mix.exs` and, in an umbrella, each `apps/*/mix.exs`.
/// Every other app that compiles or appears in a stacktrace is a dependency. Empty when
/// there is no `mix.exs` to read, and then nothing is treated as a dependency.
fn project_apps(root: &Path) -> Vec<String> {
    let Ok(mix_exs) = std::fs::read_to_string(root.join("mix.exs")) else {
        return Vec::new();
    };
    let mut apps: Vec<String> = APP_NAME_RE
        .captures_iter(&mix_exs)
        .map(|c| c[1].to_string())
        .collect();
    if let Some(dir) = APPS_PATH_RE.captures(&mix_exs)
        && let Ok(entries) = std::fs::read_dir(root.join(&dir[1]))
    {
        for entry in entries.flatten() {
            if let Ok(child) = std::fs::read_to_string(entry.path().join("mix.exs")) {
                apps.extend(APP_NAME_RE.captures_iter(&child).map(|c| c[1].to_string()));
            }
        }
    }
    apps
}

struct Filtered {
    text: String,
    /// Something beyond progress noise was cut, so the output needs a recall hint.
    cut: bool,
}

#[derive(Default)]
struct Hidden {
    dep_sections: usize,
    dep_lines: usize,
    log_lines: usize,
    repeated_logs: usize,
    dep_frames: usize,
    repeated_errors: usize,
}

impl Hidden {
    fn summary(&self) -> Option<String> {
        let parts: Vec<String> = [
            (self.dep_sections > 0).then(|| {
                format!(
                    "{} dependency compiles ({} lines)",
                    self.dep_sections, self.dep_lines
                )
            }),
            (self.log_lines > 0).then(|| format!("{} debug/info log lines", self.log_lines)),
            (self.repeated_logs > 0).then(|| format!("{} repeated log lines", self.repeated_logs)),
            (self.dep_frames > 0).then(|| format!("{} dependency stack frames", self.dep_frames)),
            (self.repeated_errors > 0).then(|| format!("{} repeated errors", self.repeated_errors)),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| format!("[rtk: hid {}]", parts.join(", ")))
    }
}

/// Filters mix output. With `cut` false, only progress noise and blank-line runs go.
fn filter_output(raw: &str, apps: &[String], cut: bool) -> Filtered {
    let mut f = Filter {
        apps,
        cut,
        out: Vec::new(),
        hidden: Hidden::default(),
        dep: None,
        logs: HashMap::new(),
        failure: Vec::new(),
        signatures: HashMap::new(),
    };
    for line in raw.lines() {
        f.feed(line);
    }
    f.flush_dep();
    f.flush_failure();

    for (index, count) in f.logs.values() {
        if *count > 0 {
            f.out[*index].push_str(&format!(" [+{count} more like this]"));
        }
    }
    let summary = f.hidden.summary();
    let mut text = collapse_blank_lines(&f.out);
    if let Some(summary) = &summary {
        text.push_str(summary);
        text.push('\n');
    }
    Filtered {
        text,
        cut: summary.is_some(),
    }
}

struct DepSection {
    name: String,
    /// rebar3's `===> Compiling dep` prints no `Generated dep app` to end on.
    rebar: bool,
    lines: Vec<String>,
    failed: bool,
}

impl DepSection {
    fn ends_before(&self, line: &str) -> bool {
        line.starts_with("Running ExUnit")
            || (self.rebar && line.starts_with("Compiling ") && PROGRESS_RE.is_match(line))
    }
}

struct Filter<'a> {
    apps: &'a [String],
    cut: bool,
    out: Vec<String>,
    hidden: Hidden,
    /// The dependency section being read.
    dep: Option<DepSection>,
    /// First output index and repeat count of each warning/error log message.
    logs: HashMap<String, (usize, usize)>,
    /// The ExUnit failure block being read.
    failure: Vec<String>,
    /// Failure number of the first failure with each error and stacktrace.
    signatures: HashMap<String, String>,
}

impl Filter<'_> {
    fn is_dependency(&self, app: &str) -> bool {
        !self.apps.is_empty() && !self.apps.iter().any(|a| a == app)
    }

    fn feed(&mut self, line: &str) {
        if !self.failure.is_empty() {
            if line.trim().is_empty() || line.starts_with("    ") {
                self.failure.push(line.to_string());
                return;
            }
            self.flush_failure();
        }

        let section = SECTION_RE.captures(line).map(|c| c[1].to_string());
        let rebar_dep = REBAR_COMPILE_RE.captures(line).map(|c| c[1].to_string());
        if section.is_some() || rebar_dep.is_some() || line.starts_with("===> ") {
            self.flush_dep();
        }
        if self.dep.as_ref().is_some_and(|dep| dep.ends_before(line)) {
            self.flush_dep();
        }
        if let Some(dep) = &mut self.dep {
            dep.lines.push(line.to_string());
            dep.failed |= DEP_FAILURE_RE.is_match(line);
            let generated = line
                .strip_prefix("Generated ")
                .and_then(|rest| rest.strip_suffix(" app"));
            if generated == Some(dep.name.as_str()) {
                self.flush_dep();
            }
            return;
        }
        if self.cut {
            // A mix project never builds with rebar3 itself, so a rebar3 compile is a dependency.
            let dep = match (&section, &rebar_dep) {
                (Some(app), _) if self.is_dependency(app) => Some((app.clone(), false)),
                (_, Some(app)) => Some((app.clone(), true)),
                _ => None,
            };
            if let Some((name, rebar)) = dep {
                self.dep = Some(DepSection {
                    name,
                    rebar,
                    lines: vec![line.to_string()],
                    failed: false,
                });
                return;
            }
            if section.is_some() {
                // The project's own section header: what follows is its compile output.
                return;
            }
        }

        if PROGRESS_RE.is_match(line) {
            return;
        }
        if FAILURE_RE.is_match(line) {
            self.failure.push(line.to_string());
            return;
        }
        if let Some(log) = LOG_RE.captures(line) {
            self.feed_log(&log[1], &log[2], &log[3]);
            return;
        }
        self.out.push(line.to_string());
    }

    fn feed_log(&mut self, line: &str, level: &str, message: &str) {
        if !self.cut {
            self.out.push(line.to_string());
            return;
        }
        if matches!(level, "debug" | "info" | "notice") {
            self.hidden.log_lines += 1;
            return;
        }
        let key = format!("[{level}] {message}");
        if let Some((_, count)) = self.logs.get_mut(&key) {
            *count += 1;
            self.hidden.repeated_logs += 1;
            return;
        }
        self.logs.insert(key, (self.out.len(), 0));
        self.out.push(line.to_string());
    }

    fn flush_dep(&mut self) {
        let Some(dep) = self.dep.take() else {
            return;
        };
        if dep.failed {
            self.out.extend(dep.lines);
        } else {
            self.hidden.dep_sections += 1;
            self.hidden.dep_lines += dep.lines.len();
        }
    }

    fn flush_failure(&mut self) {
        let mut block = std::mem::take(&mut self.failure);
        if block.is_empty() {
            return;
        }
        // Trailing blank lines separate failures; they are not part of this one.
        let trailing = block
            .iter()
            .rev()
            .take_while(|l| l.trim().is_empty())
            .count();
        block.truncate(block.len() - trailing);
        if !self.cut {
            self.out.extend(block);
            self.out.push(String::new());
            return;
        }

        let number = FAILURE_RE
            .captures(&block[0])
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let head_len = if block.get(1).is_some_and(|l| LOCATION_RE.is_match(l)) {
            2
        } else {
            1
        };
        let stack_at = block.iter().position(|l| l.trim() == "stacktrace:");
        let (details, frames) = match stack_at {
            Some(at) => (&block[head_len..at], &block[at + 1..]),
            None => (&block[head_len..], &block[block.len()..]),
        };
        // Tests that hit the same bug differ only in the code they ran and their own lines.
        let (code, error) = split_code(details);
        let test_lines: Vec<&str> = frames
            .iter()
            .filter_map(|f| TEST_FRAME_RE.captures(f).and_then(|c| c.get(1)))
            .map(|m| m.as_str())
            .collect();
        let signature: Vec<&str> = error
            .into_iter()
            .chain(frames.iter().filter(|f| {
                (APP_FRAME_RE.is_match(f) || PATH_FRAME_RE.is_match(f))
                    && !TEST_FRAME_RE.is_match(f)
            }))
            .map(String::as_str)
            .collect();

        self.out.extend(block[..head_len].iter().cloned());
        if !signature.is_empty() {
            let signature = signature.join("\n");
            if let Some(first) = self.signatures.get(&signature) {
                let note = if test_lines.is_empty() {
                    format!("(same error as {first})")
                } else {
                    format!("(same error as {first}, from {})", test_lines.join(", "))
                };
                self.out.extend(code.into_iter().cloned());
                self.out.push(format!("{}{note}", " ".repeat(5)));
                self.out.push(String::new());
                self.hidden.repeated_errors += 1;
                return;
            }
            self.signatures.insert(signature, number);
        }
        self.out.extend(details.iter().cloned());
        if let Some(at) = stack_at {
            self.out.push(block[at].clone());
            let folded = self.fold_frames(frames);
            self.out.extend(folded);
        }
        self.out.push(String::new());
    }

    /// Folds each run of two or more dependency frames into one line naming the dependencies.
    fn fold_frames(&mut self, frames: &[String]) -> Vec<String> {
        let mut folded = Vec::new();
        let mut run: Vec<&String> = Vec::new();
        for frame in frames {
            if self.dependency_of(frame).is_some() {
                run.push(frame);
                continue;
            }
            self.fold_run(&mut run, &mut folded);
            folded.push(frame.clone());
        }
        self.fold_run(&mut run, &mut folded);
        folded
    }

    fn dependency_of<'f>(&self, frame: &'f str) -> Option<&'f str> {
        let app = APP_FRAME_RE.captures(frame)?.get(2)?.as_str();
        self.is_dependency(app).then_some(app)
    }

    fn fold_run(&mut self, run: &mut Vec<&String>, into: &mut Vec<String>) {
        // One frame costs no more than the line that would replace it.
        if run.len() < 2 {
            into.extend(run.drain(..).cloned());
            return;
        }
        let indent = APP_FRAME_RE
            .captures(run[0])
            .map(|c| c[1].to_string())
            .unwrap_or_default();
        let mut deps: Vec<&str> = Vec::new();
        for frame in run.iter() {
            if let Some(app) = self.dependency_of(frame)
                && !deps.contains(&app)
            {
                deps.push(app);
            }
        }
        into.push(format!(
            "{indent}… {} frames in {}",
            run.len(),
            deps.join(", ")
        ));
        self.hidden.dep_frames += run.len();
        run.clear();
    }
}

/// Splits a failure's details into its `code:` lines (with their continuation lines, indented
/// further) and the rest.
fn split_code(details: &[String]) -> (Vec<&String>, Vec<&String>) {
    let indent = |line: &str| line.len() - line.trim_start().len();
    let mut code = Vec::new();
    let mut rest = Vec::new();
    let mut code_indent = None;
    for line in details {
        if line.trim_start().starts_with("code:") {
            code_indent = Some(indent(line));
            code.push(line);
        } else if code_indent.is_some_and(|at| !line.trim().is_empty() && indent(line) > at) {
            code.push(line);
        } else {
            code_indent = None;
            rest.push(line);
        }
    }
    (code, rest)
}

/// Joins `lines`, keeping at most one blank line in a row and none at the start.
fn collapse_blank_lines(lines: &[String]) -> String {
    let mut text = String::new();
    let mut blank = true;
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && blank {
            continue;
        }
        blank = is_blank;
        text.push_str(if is_blank { "" } else { line });
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAILURES: &str = include_str!("../../../tests/fixtures/mix_test_failures_raw.txt");
    const PRECOMMIT: &str = include_str!("../../../tests/fixtures/mix_precommit_raw.txt");

    fn apps() -> Vec<String> {
        vec!["shop".to_string()]
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn failure_numbers(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|l| FAILURE_RE.captures(l).map(|c| c[1].to_string()))
            .collect()
    }

    #[test]
    fn handles_only_mix_test_and_precommit() {
        assert!(handles(&args("mix test")));
        assert!(handles(&args("mix test test/my_app_test.exs --seed 1")));
        assert!(handles(&args("/usr/local/bin/mix precommit")));
        assert!(!handles(&args("mix compile --warnings-as-errors")));
        assert!(!handles(&args("mix format")));
        assert!(!handles(&args("mix")));
        assert!(!handles(&args("remix test")));
    }

    #[test]
    fn every_failure_keeps_its_header_and_location() {
        let out = filter_output(FAILURES, &apps(), true).text;
        let expected: Vec<String> = (1..=7).map(|n| n.to_string()).collect();
        assert_eq!(failure_numbers(&out), expected);
        for line in FAILURES.lines().filter(|l| LOCATION_RE.is_match(l)) {
            assert!(out.contains(line), "lost location {line:?}");
        }
        assert!(out.contains("Result: 12/19 passed"));
        assert!(out.contains("Failed: 7 tests"));
    }

    #[test]
    fn dependency_compiles_and_their_warnings_go() {
        let out = filter_output(FAILURES, &apps(), true).text;
        assert!(!out.contains("==> "), "{out}");
        assert!(!out.contains("Generated "));
        assert!(
            !out.contains("Postgrex.Interval"),
            "a dependency's warnings are not the project's"
        );
        assert!(out.contains("function ShopWeb.Format.price/1 is undefined"));
        assert!(out.contains("Running ExUnit with seed: 318496, max_cases: 4"));
    }

    #[test]
    fn debug_and_info_log_lines_go() {
        let raw = "Running ExUnit with seed: 1, max_cases: 4\n\n18:00:06.719 [info] GET /products\n..18:00:06.720 [debug] QUERY OK source=\"products\" db=0.1ms\n18:00:06.721 [info] Sent 200 in 2ms\n\nFinished in 0.1 seconds\n";
        let out = filter_output(raw, &apps(), true);
        assert!(!out.text.contains("[info]") && !out.text.contains("[debug]"));
        assert!(
            out.text.contains("[rtk: hid 3 debug/info log lines]"),
            "{}",
            out.text
        );
    }

    #[test]
    fn dependency_frames_fold_and_project_frames_stay() {
        let out = filter_output(FAILURES, &apps(), true).text;
        assert!(
            out.contains("       … 18 frames in phoenix_live_view, elixir"),
            "{out}"
        );
        assert!(out.contains(
            "       (shop 0.1.0) lib/shop_web/live/product_live/show.ex:25: anonymous fn/2 in ShopWeb.ProductLive.Show.render/1"
        ));
        assert!(!out.contains("(phoenix_live_view 1.2.13) lib/phoenix_live_view/diff.ex:647"));
    }

    #[test]
    fn a_repeated_failure_points_at_the_first() {
        let out = filter_output(FAILURES, &apps(), true).text;
        let after = |header: &str| -> Vec<String> {
            let rest = out.split(header).nth(1).unwrap();
            rest.lines().skip(1).take(2).map(String::from).collect()
        };
        assert_eq!(
            after("  4) test Show updates product and returns to show"),
            [
                "     test/shop_web/live/product_live_test.exs:96",
                "     (same error as 2)"
            ]
        );
        assert_eq!(
            after("  7) test Index lists all products"),
            [
                "     test/shop_web/live/product_live_test.exs:19",
                "     (same error as 3)"
            ]
        );
    }

    #[test]
    fn a_repeated_error_from_other_test_code_keeps_that_code_and_its_test_line() {
        let failure = |n: usize, test_line: usize, path: &str| {
            format!(
                "  {n}) test page {n} (MyAppWeb.TenantIndexTest)\n     test/my_app_web/tenant_index_test.exs:{}\n     ** (UndefinedFunctionError) function MyAppWeb.Format.number/1 is undefined (module MyAppWeb.Format is not available)\n     code: {{:ok, view, _html}} = live(conn, ~p\"{path}\")\n     stacktrace:\n       MyAppWeb.Format.number(1)\n       (my_app 0.1.0) lib/my_app_web/live/tenant_index.ex:223: anonymous fn/3 in MyAppWeb.TenantIndex.render/1\n       (phoenix 1.8.5) lib/phoenix/test/conn_test.ex:225: Phoenix.ConnTest.dispatch/5\n       test/my_app_web/tenant_index_test.exs:{test_line}: (test)\n\n",
                test_line - 1
            )
        };
        let raw = failure(1, 13, "/tenants") + &failure(2, 31, "/tenants/new");
        let out = filter_output(&raw, &["my_app".to_string()], true).text;
        let second: Vec<&str> = out
            .split("  2) test page 2")
            .nth(1)
            .unwrap()
            .lines()
            .collect();
        assert_eq!(
            second[1..4],
            [
                "     test/my_app_web/tenant_index_test.exs:30",
                "     code: {:ok, view, _html} = live(conn, ~p\"/tenants/new\")",
                "     (same error as 1, from test/my_app_web/tenant_index_test.exs:31)",
            ]
        );
    }

    #[test]
    fn split_code_takes_the_code_line_and_its_continuation() {
        let details: Vec<String> = [
            "     ** (MatchError) no match of right hand side value: :error",
            "     code: assert {:ok, _} =",
            "             fetch(1)",
            "     left: :error",
        ]
        .map(String::from)
        .to_vec();
        let (code, rest) = split_code(&details);
        assert_eq!(code, [&details[1], &details[2]]);
        assert_eq!(rest, [&details[0], &details[3]]);
    }

    #[test]
    fn the_failing_run_shrinks_by_most_of_its_size() {
        let out = filter_output(FAILURES, &apps(), true);
        assert!(out.cut);
        assert!(
            out.text.ends_with(
                "[rtk: hid 41 dependency compiles (213 lines), 4 repeated log lines, 36 dependency stack frames, 4 repeated errors]\n"
            ),
            "{}",
            out.text.lines().last().unwrap()
        );
        assert!(
            out.text.len() * 4 < FAILURES.len(),
            "{} of {} bytes",
            out.text.len(),
            FAILURES.len()
        );
    }

    #[test]
    fn precommit_keeps_both_kinds_of_failure() {
        let out = filter_output(PRECOMMIT, &apps(), true).text;
        let expected: Vec<String> = (1..=7).map(|n| n.to_string()).collect();
        assert_eq!(failure_numbers(&out), expected);
        assert!(out.contains(r#"     left:  "some updated name""#));
        assert!(out.contains("       test/shop/catalog_test.exs:40: (test)"));
        assert!(
            out.contains("not a textual representation of an integer"),
            "{out}"
        );
        assert!(out.contains("Result: 12/19 passed"));
        assert!(
            !out.lines()
                .any(|l| !l.is_empty() && l.chars().all(|c| c == '.')),
            "progress dots go"
        );
    }

    #[test]
    fn a_repeated_warning_log_is_shown_once_with_a_count() {
        let out = filter_output(PRECOMMIT, &apps(), true).text;
        let logs: Vec<&str> = out
            .lines()
            .filter(|l| l.contains("price cache miss"))
            .collect();
        assert_eq!(
            logs,
            ["18:00:12.283 [warning] price cache miss [+4 more like this]"]
        );
    }

    #[test]
    fn a_dependency_that_fails_to_compile_is_shown_whole() {
        let raw = "==> broken_dep\nCompiling 3 files (.ex)\n** (CompileError) lib/broken.ex:3: undefined function foo/0\ncould not compile dependency :broken_dep\n";
        let out = filter_output(raw, &apps(), true).text;
        assert!(out.contains("==> broken_dep"));
        assert!(out.contains("could not compile dependency :broken_dep"));
    }

    #[test]
    fn without_cutting_only_progress_noise_goes() {
        let out = filter_output(FAILURES, &apps(), false);
        assert!(!out.cut);
        let count = |text: &str, needle: &str| text.matches(needle).count();
        for kept in [
            "(phoenix_live_view 1.2.13)",
            "[warning] price cache miss",
            "Postgrex.Interval",
        ] {
            assert_eq!(count(&out.text, kept), count(FAILURES, kept), "{kept}");
        }
        // rebar3's `===> Analyzing applications...` is progress, so count mix's headers only.
        let sections = |text: &str| text.lines().filter(|l| l.starts_with("==> ")).count();
        assert_eq!(sections(&out.text), sections(FAILURES));
        assert!(!out.text.contains("Generated shop app"));
    }

    #[test]
    fn without_mix_exs_no_app_counts_as_a_dependency() {
        let out = filter_output(FAILURES, &[], true).text;
        assert!(out.contains("(phoenix_live_view 1.2.13) lib/phoenix_live_view/diff.ex:647"));
        assert!(!out.contains(" frames in "));
    }

    #[test]
    fn a_rebar3_section_ends_where_the_tests_start() {
        let raw = "===> Compiling gproc\n     ┌─ src/gproc.erl:\nRunning ExUnit with seed: 1, max_cases: 4\n\nFinished in 0.1 seconds\n";
        let out = filter_output(raw, &apps(), true).text;
        assert!(!out.contains("gproc"));
        assert!(
            out.starts_with("Running ExUnit with seed: 1, max_cases: 4\n"),
            "{out}"
        );
        assert!(out.contains("Finished in 0.1 seconds"));
    }

    #[test]
    fn project_apps_reads_mix_exs_and_umbrella_apps() -> Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("mix.exs"),
            "defmodule Umbrella.MixProject do\n  def project do\n    [\n      apps_path: \"apps\",\n      version: \"0.1.0\"\n    ]\n  end\nend\n",
        )?;
        for app in ["shop", "shop_web"] {
            let path = dir.path().join("apps").join(app);
            std::fs::create_dir_all(&path)?;
            std::fs::write(
                path.join("mix.exs"),
                format!("  def project do\n    [\n      app: :{app},\n    ]\n  end\n"),
            )?;
        }
        let mut found = project_apps(dir.path());
        found.sort();
        assert_eq!(found, ["shop", "shop_web"]);
        assert!(project_apps(&dir.path().join("missing")).is_empty());
        Ok(())
    }
}

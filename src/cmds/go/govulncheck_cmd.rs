//! Filters govulncheck output: runs it with `-format json` and prints one block per affected module.
//!
//! The JSON stream carries the full OSV record of every vulnerability in the dependency graph (hundreds of
//! KB), while only symbol-level findings -- vulnerable code your code actually calls -- are actionable.
//! Those are kept with their first call site; package- and module-level findings become a count.

use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use serde::Deserialize;
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;

/// govulncheck's text mode exits 3 when vulnerabilities affect the code; JSON mode always exits 0.
const EXIT_VULNERABLE: i32 = 3;

#[derive(Deserialize)]
struct Message {
    osv: Option<Osv>,
    finding: Option<Finding>,
}

#[derive(Deserialize)]
struct Osv {
    id: String,
    #[serde(default)]
    summary: String,
}

#[derive(Deserialize)]
struct Finding {
    osv: String,
    fixed_version: Option<String>,
    #[serde(default)]
    trace: Vec<Frame>,
}

#[derive(Deserialize)]
struct Frame {
    module: String,
    version: Option<String>,
    package: Option<String>,
    function: Option<String>,
    receiver: Option<String>,
    position: Option<Position>,
}

#[derive(Deserialize)]
struct Position {
    filename: String,
    line: u32,
    column: u32,
}

/// govulncheck's flags (Go `flag` package) that take a value, from `govulncheck -h` (v1.8).
fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    (kind == TokenKind::Long
        && matches!(
            name,
            "C" | "db" | "format" | "mode" | "scan" | "show" | "tags"
        ))
    .then(ValueSpec::value)
}

/// Pass through when the user picked an output format or detail level, a non-default scan or mode
/// (they change what "affected" means), or asked for help/version.
fn is_passthrough(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &takes_value, Dialect::GoFlag);
    tokens.iter().any(|t| {
        t.kind == TokenKind::Long
            && matches!(
                t.text,
                "json" | "format" | "show" | "scan" | "mode" | "version" | "h" | "help"
            )
    })
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = &args_utils::restore_double_dash(args);

    if is_passthrough(args) {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("govulncheck", &os_args, verbose);
    }

    let mut cmd = resolved_command("govulncheck");
    cmd.args(["-format", "json"]);
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: govulncheck -format json {}", args.join(" "));
    }

    let vulnerable = Cell::new(false);
    let code = runner::run_filtered_with_exit(
        cmd,
        "govulncheck",
        &display_args(args),
        |stdout, exit_code| match filter_govulncheck_json(stdout) {
            // A failed scan (no go.mod, build errors, no network) explains itself on stderr, which the runner
            // forwards; a summary built from the partial stream would claim the code is clean.
            _ if exit_code != 0 => String::new(),
            Some((text, affected)) => {
                vulnerable.set(affected > 0);
                text
            }
            // Not a JSON stream (e.g. an old govulncheck): show it unchanged.
            None => stdout.to_string(),
        },
        runner::RunOptions::stdout_only().tee("govulncheck"),
    )?;

    Ok(if code == 0 && vulnerable.get() {
        EXIT_VULNERABLE
    } else {
        code
    })
}

#[derive(PartialEq, PartialOrd, Clone, Copy)]
enum Level {
    Module,
    Package,
    Symbol,
}

fn level(f: &Finding) -> Level {
    match f.trace.first() {
        Some(fr) if fr.function.is_some() => Level::Symbol,
        Some(fr) if fr.package.is_some() => Level::Package,
        _ => Level::Module,
    }
}

/// The package name as Go code refers to it, guessed from its import path the way govulncheck prints it:
/// `golang.org/x/text/language` -> `language`, `gopkg.in/yaml.v2` -> `yaml`, `github.com/a/b/v3` -> `b`.
fn package_name(path: &str) -> &str {
    let mut segments = path.rsplit('/');
    let mut last = segments.next().unwrap_or(path);
    if is_major_version(last) {
        last = segments.next().unwrap_or(last);
    }
    match last.rsplit_once(".v") {
        Some((name, major)) if !major.is_empty() && major.chars().all(|c| c.is_ascii_digit()) => {
            name
        }
        _ => last,
    }
}

fn is_major_version(segment: &str) -> bool {
    segment.len() > 1
        && segment.starts_with('v')
        && segment[1..].chars().all(|c| c.is_ascii_digit())
}

/// `example.com/foo/bar` + `Baz` -> `bar.Baz`; with a receiver -> `bar.(*T).Baz`, as govulncheck prints it.
fn symbol(frame: &Frame) -> String {
    let pkg = package_name(frame.package.as_deref().unwrap_or(&frame.module));
    let func = frame.function.as_deref().unwrap_or("");
    match frame.receiver.as_deref() {
        Some(r) if r.starts_with('*') => format!("{pkg}.({r}).{func}"),
        Some(r) => format!("{pkg}.{r}.{func}"),
        None => format!("{pkg}.{func}"),
    }
}

/// `main.go:11:44 vulnsample.main calls language.ParseAcceptLanguage`
fn call_site(f: &Finding) -> Option<String> {
    let callee = f.trace.first()?;
    let caller = f.trace.last()?;
    let pos = caller.position.as_ref()?;
    Some(format!(
        "{}:{}:{} {} calls {}",
        pos.filename,
        pos.line,
        pos.column,
        symbol(caller),
        symbol(callee)
    ))
}

/// Orders Go module versions (`v1.2.10` > `v1.2.9`) by their numeric release parts; good enough to pick
/// the highest fixed version, which is all this is used for.
fn version_key(v: &str) -> Vec<u64> {
    let release = v
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    release.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Returns the compact report and the number of vulnerabilities that affect the code,
/// or `None` if `output` is not a govulncheck JSON stream.
fn filter_govulncheck_json(output: &str) -> Option<(String, usize)> {
    let mut summaries: HashMap<String, String> = HashMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut parsed_any = false;

    for msg in serde_json::Deserializer::from_str(output).into_iter::<Message>() {
        let Ok(msg) = msg else { break };
        parsed_any = true;
        if let Some(osv) = msg.osv {
            summaries.insert(osv.id, osv.summary);
        }
        if let Some(f) = msg.finding {
            findings.push(f);
        }
    }
    if !parsed_any {
        return None;
    }

    // A vulnerability is reported once, at the most precise level any of its findings reached.
    let mut best: HashMap<&str, Level> = HashMap::new();
    for f in &findings {
        let l = level(f);
        let e = best.entry(f.osv.as_str()).or_insert(l);
        if l > *e {
            *e = l;
        }
    }

    struct ModuleReport<'a> {
        version: String,
        fixed: Option<&'a str>,
        vulns: BTreeMap<&'a str, Vec<String>>, // id -> call sites
    }
    let mut modules: BTreeMap<&str, ModuleReport> = BTreeMap::new();
    for f in findings.iter().filter(|f| level(f) == Level::Symbol) {
        let Some(frame) = f.trace.first() else {
            continue;
        };
        let m = modules
            .entry(frame.module.as_str())
            .or_insert_with(|| ModuleReport {
                version: frame.version.clone().unwrap_or_default(),
                fixed: None,
                vulns: BTreeMap::new(),
            });
        if let Some(fixed) = f.fixed_version.as_deref()
            && m.fixed
                .is_none_or(|cur| version_key(fixed) > version_key(cur))
        {
            m.fixed = Some(fixed);
        }
        let sites = m.vulns.entry(f.osv.as_str()).or_default();
        if let Some(site) = call_site(f)
            && !sites.contains(&site)
        {
            sites.push(site);
        }
    }

    let affected = best.values().filter(|l| **l == Level::Symbol).count();
    let imported = best.values().filter(|l| **l == Level::Package).count();
    let required = best.values().filter(|l| **l == Level::Module).count();

    let mut out = String::new();
    if affected == 0 {
        out.push_str("govulncheck: no vulnerabilities affect your code\n");
    } else {
        out.push_str(&format!(
            "govulncheck: {affected} {} affect your code ({} {})\n",
            if affected == 1 {
                "vulnerability"
            } else {
                "vulnerabilities"
            },
            modules.len(),
            if modules.len() == 1 {
                "module"
            } else {
                "modules"
            }
        ));
        for (module, m) in &modules {
            let fix = m
                .fixed
                .map_or("no fix yet".to_string(), |v| format!("upgrade to {v}"));
            out.push_str(&format!("\n{module}@{} -> {fix}\n", m.version));
            for (id, sites) in &m.vulns {
                let summary = summaries.get(*id).map(String::as_str).unwrap_or("");
                out.push_str(&format!("  {id} {summary}\n"));
                if let Some(first) = sites.first() {
                    out.push_str(&format!("    {first}\n"));
                }
                if sites.len() > 1 {
                    out.push_str(&format!("    +{} more call sites\n", sites.len() - 1));
                }
            }
        }
    }
    if imported + required > 0 {
        out.push_str(&format!(
            "\nNot called by your code: {imported} in imported packages, {required} in required modules \
(rtk proxy govulncheck -show verbose ./... for details)\n"
        ));
    }
    Some((out, affected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const FIXTURE: &str = include_str!("../../../tests/fixtures/govulncheck_json_raw.json");

    #[test]
    fn test_passthrough_detection() {
        for case in [
            &["-json", "./..."][..],
            &["-format", "sarif", "./..."],
            &["--format=text"],
            &["-show", "verbose", "./..."],
            &["-scan=module"],
            &["-mode", "binary", "app"],
            &["-version"],
            &["-h"],
        ] {
            assert!(is_passthrough(&args(case)), "{case:?} should pass through");
        }
        for case in [
            &["./..."][..],
            &["-test", "./..."],
            &["-C", "sub", "./..."],
            &["-tags", "integration", "./..."],
        ] {
            assert!(!is_passthrough(&args(case)), "{case:?} should be filtered");
        }
    }

    #[test]
    fn test_package_name() {
        assert_eq!(package_name("golang.org/x/text/language"), "language");
        assert_eq!(package_name("gopkg.in/yaml.v2"), "yaml");
        assert_eq!(package_name("github.com/jackc/pgx/v5"), "pgx");
        assert_eq!(package_name("example.com/vulnsample"), "vulnsample");
        assert_eq!(package_name("github.com/a/semver.vfoo"), "semver.vfoo");
    }

    #[test]
    fn test_version_key_orders_numerically() {
        assert!(version_key("v2.2.10") > version_key("v2.2.9"));
        assert!(version_key("v0.3.8") > version_key("v0.3.7"));
        assert!(version_key("v1.0.0-rc.1") == version_key("v1.0.0"));
    }

    #[test]
    fn test_real_output() {
        let (out, affected) = filter_govulncheck_json(FIXTURE).expect("fixture is a JSON stream");
        assert_eq!(affected, 4, "{out}");
        assert!(
            out.starts_with("govulncheck: 4 vulnerabilities affect your code (2 modules)"),
            "{out}"
        );
        // Highest fixed version across the module's vulnerabilities.
        assert!(
            out.contains("gopkg.in/yaml.v2@v2.2.2 -> upgrade to v2.2.8"),
            "{out}"
        );
        assert!(
            out.contains("golang.org/x/text@v0.3.7 -> upgrade to v0.3.8"),
            "{out}"
        );
        assert!(
            out.contains("main.go:11:44 vulnsample.main calls language.ParseAcceptLanguage"),
            "{out}"
        );
        assert!(
            out.contains("main.go:13:20 vulnsample.main calls yaml.Unmarshal"),
            "{out}"
        );
        assert!(
            out.contains("1 in imported packages, 14 in required modules"),
            "{out}"
        );
    }

    #[test]
    fn test_savings() {
        let (out, _) = filter_govulncheck_json(FIXTURE).unwrap();
        let savings =
            100.0 - estimate_tokens(&out) as f64 / estimate_tokens(FIXTURE) as f64 * 100.0;
        assert!(
            savings >= 90.0,
            "expected >= 90% savings, got {savings:.1}%"
        );
    }

    #[test]
    fn test_clean_and_garbage() {
        let clean = r#"{"config":{"scanner_name":"govulncheck"}}
{"progress":{"message":"Scanning your code..."}}"#;
        let (out, affected) = filter_govulncheck_json(clean).unwrap();
        assert_eq!(affected, 0);
        assert_eq!(out, "govulncheck: no vulnerabilities affect your code\n");
        assert!(filter_govulncheck_json("govulncheck: loading packages: no go.mod").is_none());
    }
}

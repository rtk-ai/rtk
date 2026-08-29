//! SSH client output compression.
//!
//! `ssh` is one of the highest-volume commands in agent workflows, yet almost
//! none of it is compressed today. This wrapper splits ssh into two paths:
//!
//! * **Interactive** (stdout is a TTY, an interactive login, `-t`, `-N`, or a
//!   subsystem request) — exec ssh with stdin/stdout/stderr inherited and **no
//!   filtering**. Touching an interactive session would break the terminal, so
//!   we never do.
//! * **Command mode** (a remote command with no PTY, output going to a pipe) —
//!   compress the payload: strip ssh connection banners/login noise, collapse
//!   blank runs, fold long runs of identical lines, and truncate pathological
//!   lines. Exit code and the failure path are preserved faithfully.
//!
//! When in doubt the wrapper favours passthrough — a wrapper that eats real
//! output is worse than no wrapper at all.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, strip_ansi, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;
use std::io::IsTerminal;

/// Cap a single line at this many characters. Deliberately generous: ssh
/// command output is arbitrary remote text, so we only clip genuinely
/// pathological lines (minified blobs, base64) and pass everything else intact.
const MAX_LINE_WIDTH: usize = 500;

/// Collapse a run of identical consecutive lines only once it reaches this
/// length. Runs of 2 are kept verbatim — folding them saves nothing and risks
/// hiding meaningful repetition.
const DEDUP_RUN_THRESHOLD: usize = 3;

/// ssh short options that take a separate argument (`-p 22`, `-o Foo=bar`, ...).
/// Used to walk past option values when locating the destination and remote
/// command. Source: ssh(1).
const OPTS_WITH_ARG: &str = "BbcDEeFIiJLlmOopQRSWw";

lazy_static! {
    /// ssh-layer connection noise. Anchored so it never matches real command
    /// output — these are lines ssh itself emits around the remote payload.
    static ref NOISE_PATTERNS: Vec<Regex> = [
        r"^Warning: Permanently added ",
        r"^Connection to \S+ closed\.?$",
        r"^Connection to \S+ closed by remote host\.?$",
        r"^Shared connection to \S+ closed\.?$",
        r"^Authenticated to \S+",
        r"^debug\d+: ",
        r"^OpenSSH_\S+",
        r"^Pseudo-terminal will not be allocated",
        r"^X11 forwarding request failed",
        r"^Last login: ",
        r"^Killed by signal \d+\.?$",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static ssh noise pattern must compile"))
    .collect();
}

/// Run `ssh` with output compression in command mode and full passthrough for
/// interactive sessions.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    run_ssh("ssh", args, verbose, std::io::stdout().is_terminal())
}

/// Testable core: `program` is injectable so exit-code and dispatch behaviour
/// can be exercised without a live ssh server.
fn run_ssh(program: &str, args: &[String], verbose: u8, stdout_is_tty: bool) -> Result<i32> {
    if is_interactive(args, stdout_is_tty) {
        // Interactive: inherit all three streams, filter nothing, forward the
        // exit code verbatim.
        if verbose > 0 {
            eprintln!("ssh interactive passthrough: {}", args.join(" "));
        }
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(program, &os_args, verbose);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", program, args.join(" "));
    }

    let mut cmd = resolved_command(program);
    for arg in args {
        cmd.arg(arg);
    }

    // Combined text output: stripping banners means reading ssh's stderr, so we
    // filter the merged stream. `early_exit_on_failure` keeps the failure path
    // byte-for-byte (stdout and stderr preserved separately) for debugging, and
    // `inherit_stdin` keeps `cmd | rtk ssh host 'cat > f'` working.
    runner::run_filtered(
        cmd,
        "ssh",
        &args.join(" "),
        filter_ssh_output,
        RunOptions::default()
            .tee("ssh")
            .early_exit_on_failure()
            .inherit_stdin(),
    )
}

/// Decide whether an ssh invocation must be passed through untouched.
///
/// Conservative by design: any doubt resolves to interactive (passthrough).
/// Returns `true` when:
/// * stdout is a TTY (a human at a terminal), or
/// * a PTY is forced (`-t`/`-tt`), or
/// * no remote command is executed (`-N`, or destination with no trailing
///   command — an interactive login), or
/// * a subsystem is requested (`-s`, e.g. sftp — possibly binary), or
/// * no destination can be found (malformed / `ssh` alone).
fn is_interactive(args: &[String], stdout_is_tty: bool) -> bool {
    if stdout_is_tty {
        return true;
    }

    let mut force_tty = false;
    let mut no_command = false;
    let mut subsystem = false;
    let mut destination_seen = false;
    let mut remote_command_seen = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        if destination_seen {
            // First token after the destination begins the remote command.
            remote_command_seen = true;
            break;
        }

        if arg.starts_with('-') && arg.len() > 1 {
            let chars: Vec<char> = arg[1..].chars().collect();
            let mut j = 0;
            while j < chars.len() {
                match chars[j] {
                    't' => force_tty = true,
                    'N' => no_command = true,
                    's' => subsystem = true,
                    c if OPTS_WITH_ARG.contains(c) => {
                        // Value is either attached (`-p22`) or the next token
                        // (`-p 22`). Either way, stop scanning this token.
                        if j + 1 == chars.len() {
                            i += 1; // consume the separate argument token
                        }
                        break;
                    }
                    _ => {}
                }
                j += 1;
            }
        } else {
            // First non-option token is the destination (user@host / URI).
            destination_seen = true;
        }

        i += 1;
    }

    force_tty || no_command || subsystem || !remote_command_seen
}

/// Compress command-mode ssh output.
fn filter_ssh_output(output: &str) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    let cleaned = strip_ansi(output);

    // Pass 1: drop ssh connection noise, collapse blank runs, clip wide lines.
    let mut lines: Vec<String> = Vec::new();
    let mut prev_blank = false;
    for line in cleaned.lines() {
        if is_noise(line) {
            continue;
        }
        if line.trim().is_empty() {
            if prev_blank {
                continue;
            }
            prev_blank = true;
            lines.push(String::new());
            continue;
        }
        prev_blank = false;
        lines.push(truncate(line, MAX_LINE_WIDTH));
    }

    // Trim blank lines left at the edges after stripping banners.
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    // Pass 2: fold long runs of identical consecutive lines.
    fold_repeats(&lines)
}

fn is_noise(line: &str) -> bool {
    NOISE_PATTERNS.iter().any(|re| re.is_match(line))
}

/// Keep one copy of each run of identical lines; annotate runs of
/// [`DEDUP_RUN_THRESHOLD`]+ with a recoverable count. Blank lines are never
/// annotated (they were already collapsed in pass 1).
fn fold_repeats(lines: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        let mut run = 1;
        while i + run < lines.len() && &lines[i + run] == line {
            run += 1;
        }

        out.push(line.clone());
        if !line.is_empty() && run >= DEDUP_RUN_THRESHOLD {
            out.push(format!("    ... [×{} identical]", run));
        } else if run > 1 {
            // Below threshold: preserve the duplicates verbatim.
            for _ in 1..run {
                out.push(line.clone());
            }
        }

        i += run;
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_interactive: dispatch decision ---------------------------------

    #[test]
    fn test_interactive_when_stdout_is_tty() {
        // A human at a terminal: never filter, even with a remote command.
        assert!(is_interactive(&["host".into(), "ls".into()], true));
    }

    #[test]
    fn test_interactive_login_has_no_command() {
        // `ssh host` — interactive login shell.
        assert!(is_interactive(&["host".into()], false));
    }

    #[test]
    fn test_interactive_user_at_host_no_command() {
        assert!(is_interactive(&["user@host".into()], false));
    }

    #[test]
    fn test_command_mode_is_not_interactive() {
        // `ssh host ls -la` with output going to a pipe → compress.
        assert!(!is_interactive(
            &["host".into(), "ls".into(), "-la".into()],
            false
        ));
    }

    #[test]
    fn test_force_tty_is_interactive() {
        assert!(is_interactive(&["-t".into(), "host".into(), "top".into()], false));
        assert!(is_interactive(&["-tt".into(), "host".into(), "top".into()], false));
    }

    #[test]
    fn test_no_command_flag_is_interactive() {
        // Port-forward / tunnel: no remote command runs.
        assert!(is_interactive(
            &["-N".into(), "-L".into(), "8080:localhost:80".into(), "host".into()],
            false
        ));
    }

    #[test]
    fn test_subsystem_is_interactive() {
        assert!(is_interactive(&["-s".into(), "host".into(), "sftp".into()], false));
    }

    #[test]
    fn test_option_with_separate_arg_then_command() {
        // `-p 22` must not be mistaken for the destination.
        assert!(!is_interactive(
            &["-p".into(), "22".into(), "host".into(), "uptime".into()],
            false
        ));
    }

    #[test]
    fn test_option_with_separate_arg_no_command() {
        assert!(is_interactive(&["-p".into(), "22".into(), "host".into()], false));
    }

    #[test]
    fn test_option_with_attached_arg_then_command() {
        assert!(!is_interactive(&["-p22".into(), "host".into(), "uptime".into()], false));
    }

    #[test]
    fn test_multi_option_value_then_command() {
        assert!(!is_interactive(
            &["-o".into(), "StrictHostKeyChecking=no".into(), "host".into(), "id".into()],
            false
        ));
    }

    #[test]
    fn test_no_args_is_interactive() {
        assert!(is_interactive(&[], false));
    }

    // --- filter_ssh_output: banner / noise stripping -----------------------

    #[test]
    fn test_empty_passes_through() {
        assert_eq!(filter_ssh_output(""), "");
    }

    #[test]
    fn test_strips_known_hosts_banner() {
        let input = "Warning: Permanently added '10.0.0.1' (ED25519) to the list of known hosts.\ntotal 4\napp";
        let result = filter_ssh_output(input);
        assert!(!result.contains("Permanently added"));
        assert!(result.contains("total 4"));
        assert!(result.contains("app"));
    }

    #[test]
    fn test_strips_debug_and_auth_and_close_lines() {
        let input = "debug1: Connecting to host port 22.\nAuthenticated to host ([1.2.3.4]:22).\nuptime: 42 days\nConnection to host closed.";
        let result = filter_ssh_output(input);
        assert_eq!(result, "uptime: 42 days");
    }

    #[test]
    fn test_keeps_real_output_untouched() {
        let input = "line one\nline two\nline three";
        assert_eq!(filter_ssh_output(input), "line one\nline two\nline three");
    }

    #[test]
    fn test_collapses_blank_runs() {
        let input = "a\n\n\n\nb";
        assert_eq!(filter_ssh_output(input), "a\n\nb");
    }

    #[test]
    fn test_strips_ansi() {
        let input = "\x1b[31mred\x1b[0m normal";
        assert_eq!(filter_ssh_output(input), "red normal");
    }

    // --- filter_ssh_output: repeat folding ---------------------------------

    #[test]
    fn test_folds_long_identical_run() {
        let input = "start\nsame\nsame\nsame\nsame\nend";
        let result = filter_ssh_output(input);
        assert!(result.contains("start"));
        assert!(result.contains("end"));
        // One copy kept, then a single count marker.
        assert_eq!(result.matches("same").count(), 1);
        assert!(result.contains("[×4 identical]"));
    }

    #[test]
    fn test_keeps_short_identical_run_verbatim() {
        // A run of 2 is below the fold threshold.
        let input = "dup\ndup";
        let result = filter_ssh_output(input);
        assert_eq!(result, "dup\ndup");
        assert!(!result.contains("identical"));
    }

    #[test]
    fn test_truncates_pathological_line() {
        let long = "x".repeat(1000);
        let result = filter_ssh_output(&long);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= MAX_LINE_WIDTH);
    }

    // --- token savings -----------------------------------------------------

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_banner_heavy_output_saves_tokens() {
        let input = "Warning: Permanently added '192.168.1.10' (ED25519) to the list of known hosts.\n\
                     debug1: Connecting to 192.168.1.10 port 22.\n\
                     debug1: Connection established.\n\
                     Authenticated to 192.168.1.10 ([192.168.1.10]:22).\n\
                     \n\
                     service is healthy\n\
                     \n\
                     Connection to 192.168.1.10 closed.";
        let result = filter_ssh_output(input);
        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "ssh banner strip: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // --- exit-code fidelity via the shared runner --------------------------

    #[test]
    fn test_command_mode_preserves_zero_exit() {
        // `true` ignores args, exits 0; command mode path returns it faithfully.
        let code = run_ssh("true", &["host".into(), "cmd".into()], 0, false).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn test_interactive_passthrough_preserves_nonzero_exit() {
        // No remote command → passthrough; `false` exits 1, forwarded verbatim.
        let code = run_ssh("false", &["host".into()], 0, false).unwrap();
        assert_eq!(code, 1);
    }
}

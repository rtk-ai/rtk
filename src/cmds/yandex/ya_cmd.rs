//! `ya` CLI proxy — test-mode envelope + build-mode progress filter.
//!
//! `ya make -t*` / `ya test` → [`filter_ya_envelope`] via `run_streamed`
//! ([`test_stream_filter`](crate::cmds::yandex::envelope::test_stream_filter)).
//! `ya make` (no test flags) → [`filter_ya_build`] via `run_streamed`
//! ([`build_stream_filter`](crate::cmds::yandex::ya_build::build_stream_filter)).
//! User argv (`-F`, `-r`, `-ttX`, …) is forwarded unchanged — never rewritten.
//! `ya tool` / other → passthrough.

use crate::cmds::yandex::envelope::{is_test_mode, test_stream_filter};
use crate::cmds::yandex::ya_build::{build_stream_filter, is_build_mode};
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;
use std::process::Command;

/// First-token subdispatch for filter routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YaKind {
    Make,
    Test,
    Tool,
    Other,
}

/// Execution plan for `run` — testable without spawning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YaPipeline {
    /// Test-mode: `run_streamed` + tee `"ya"`.
    TestFiltered { tee: &'static str },
    /// Build-mode: `run_streamed` + tee `"ya"` (same [`YaBuildStreamFilter`] oracle).
    BuildFiltered { tee: &'static str },
    Passthrough,
}

/// Classify `ya` argv by the first positional token.
pub fn classify(args: &[String]) -> YaKind {
    match args.first().map(String::as_str) {
        Some("make") => YaKind::Make,
        Some("test") => YaKind::Test,
        Some("tool") => YaKind::Tool,
        _ => YaKind::Other,
    }
}

/// Shared routing: test → envelope; build make → build filter; else passthrough.
pub fn pipeline(args: &[String]) -> YaPipeline {
    if is_test_mode(args) {
        YaPipeline::TestFiltered { tee: "ya" }
    } else if is_build_mode(args) {
        YaPipeline::BuildFiltered { tee: "ya" }
    } else {
        YaPipeline::Passthrough
    }
}

/// Child `Command` with user argv forwarded as-is (no rewrite).
fn build_ya_command(args: &[String]) -> Command {
    let mut cmd = resolved_command("ya");
    for arg in args {
        cmd.arg(arg);
    }
    cmd
}

/// Run `ya` — filtered in test/build mode, passthrough otherwise.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let kind = classify(args);

    if verbose > 0 {
        eprintln!(
            "ya {:?} → {}",
            kind,
            match pipeline(args) {
                YaPipeline::TestFiltered { .. } => "test-mode stream filter",
                YaPipeline::BuildFiltered { .. } => "build-mode stream filter",
                YaPipeline::Passthrough => "passthrough",
            }
        );
    }

    match pipeline(args) {
        YaPipeline::TestFiltered { tee } => runner::run_streamed(
            build_ya_command(args),
            "ya",
            &args.join(" "),
            Box::new(test_stream_filter()),
            runner::RunOptions::with_tee(tee).source_hint(crate::cmds::yandex::source_lines::hint),
        ),
        YaPipeline::BuildFiltered { tee } => runner::run_streamed(
            build_ya_command(args),
            "ya",
            &args.join(" "),
            Box::new(build_stream_filter()),
            runner::RunOptions::with_tee(tee),
        ),
        YaPipeline::Passthrough => {
            let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough("ya", &os_args, verbose)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_make() {
        let args = ["make", "-t", "path"].map(String::from);
        assert_eq!(classify(&args), YaKind::Make);
    }

    #[test]
    fn classify_test() {
        assert_eq!(classify(&["test".into(), "-r".into()]), YaKind::Test);
    }

    #[test]
    fn classify_tool() {
        assert_eq!(classify(&["tool".into(), "dump_json".into()]), YaKind::Tool);
    }

    #[test]
    fn classify_empty_and_other() {
        assert_eq!(classify(&[]), YaKind::Other);
        assert_eq!(classify(&["package".into()]), YaKind::Other);
        assert_eq!(classify(&["-h".into()]), YaKind::Other);
    }

    #[test]
    fn ya_test_and_make_t_share_test_filtered_pipeline() {
        let make_t = ["make", "-t", "pay/lib/tests"].map(String::from);
        let ya_test = ["test", "-r", "pay/lib/tests"].map(String::from);
        let filtered = YaPipeline::TestFiltered { tee: "ya" };
        assert_eq!(pipeline(&make_t), filtered);
        assert_eq!(pipeline(&ya_test), filtered);
        assert_eq!(
            pipeline(&["tool", "dump"].map(String::from)),
            YaPipeline::Passthrough
        );
    }

    /// S6-T1: bare `ya make` routes to build filter (not passthrough).
    #[test]
    fn ya_make_without_test_flags_is_build_filtered() {
        assert_eq!(
            pipeline(&["make", "python", "-r"].map(String::from)),
            YaPipeline::BuildFiltered { tee: "ya" }
        );
        assert_eq!(
            pipeline(&["make", "-r", "pay/lib"].map(String::from)),
            YaPipeline::BuildFiltered { tee: "ya" }
        );
    }

    #[test]
    fn build_ya_command_forwards_filter_flags() {
        let args = [
            "test",
            "-F",
            "*order*",
            "-r",
            "pay/lib/tests",
            "-ttX",
        ]
        .map(String::from);
        let cmd = build_ya_command(&args);
        let forwarded: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            forwarded,
            vec![
                "test",
                "-F",
                "*order*",
                "-r",
                "pay/lib/tests",
                "-ttX",
            ]
        );
    }

    #[test]
    fn build_ya_command_forwards_make_ttx_and_filter() {
        let args = ["make", "-ttX", "-F", "*sku*", "pay/lib"].map(String::from);
        let cmd = build_ya_command(&args);
        let forwarded: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(forwarded, args);
        assert_eq!(pipeline(&args), YaPipeline::TestFiltered { tee: "ya" });
    }
}

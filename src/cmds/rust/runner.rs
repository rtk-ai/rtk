//! Shell-string wrappers over the shared err/test command runners in core.

use crate::core::runner::{run_err_cmd, run_test_cmd};
use anyhow::Result;
use std::process::Command;

fn contains_rtk_invocation(command: &str) -> bool {
    command
        .split(['&', '|', ';'])
        .filter_map(|segment| segment.split_whitespace().next())
        .map(|token| token.trim_matches(['"', '\'']))
        .map(|token| token.rsplit(['/', '\\']).next().unwrap_or(token))
        .any(|token| matches!(token, "rtk" | "rtk.exe"))
}

fn build_shell_command(command: &str) -> Command {
    let mut shell = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };

    // A nested RTK process must not reuse the parent integration's execution
    // id. The parent wrapper records the final shell output; sharing the id
    // would make execution_by_id add the child record a second time.
    if contains_rtk_invocation(command) {
        shell.env_remove("RTK_EXECUTION_ID");
    }

    shell
}

/// Run a command via the shell and filter output to show only errors/warnings.
pub fn run_err(command: &str, verbose: u8) -> Result<i32> {
    run_err_cmd(build_shell_command(command), "err", command, "err", verbose)
}

/// Run tests via the shell and show only failures.
pub fn run_test(command: &str, verbose: u8) -> Result<i32> {
    run_test_cmd(
        build_shell_command(command),
        "test",
        command,
        "test",
        crate::core::runner::TestEcosystem::detect(command),
        verbose,
    )
}

#[cfg(test)]
mod tests {
    use super::contains_rtk_invocation;

    #[test]
    fn detects_nested_rtk_invocations_without_matching_similar_names() {
        for command in [
            "rtk read file.txt",
            "rtk.exe read file.txt",
            r#"C:\\Tools\\rtk.exe read file.txt"#,
            "echo before && rtk rg pattern file.txt",
            "echo before & \"C:/Tools/rtk.exe\" read file.txt",
        ] {
            assert!(contains_rtk_invocation(command), "{command}");
        }

        for command in [
            "echo rtk read file.txt",
            "my-rtk-tool read file.txt",
            "rtk-helper read file.txt",
            "cargo test --package rtk",
        ] {
            assert!(!contains_rtk_invocation(command), "{command}");
        }
    }
}

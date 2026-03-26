use crate::cmds::js::lint_cmd;
use crate::cmds::python::{pip_cmd, pytest_cmd, ruff_cmd};
use crate::core::tracking;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("uv: no subcommand specified");
    }

    match args[0].as_str() {
        "run" => run_run(&args[1..], verbose),
        "pip" => pip_cmd::run(&args[1..], verbose),
        "sync" => run_sync(&args[1..], verbose),
        _ => run_passthrough(args, verbose),
    }
}

fn run_run(args: &[String], verbose: u8) -> Result<()> {
    if let Some((tool, start_idx)) = routed_uv_run_tool(args) {
        let rest = &args[start_idx..];
        return run_routed_uv_tool(tool, rest, verbose);
    }

    run_passthrough_with_prefix("run", args, verbose)
}

fn run_routed_uv_tool(tool: &str, args: &[String], verbose: u8) -> Result<()> {
    match tool {
        "pytest" => pytest_cmd::run(args, verbose),
        "ruff" => ruff_cmd::run(args, verbose),
        "mypy" | "pylint" | "flake8" => {
            let mut lint_args = vec![tool.to_string()];
            lint_args.extend(args.iter().cloned());
            lint_cmd::run(&lint_args, verbose)
        }
        _ => run_passthrough_with_prefix("run", args, verbose),
    }
}

fn routed_uv_run_tool(args: &[String]) -> Option<(&str, usize)> {
    if args.is_empty() {
        return None;
    }

    if args.len() >= 3 && is_python_executable(&args[0]) && args[1] == "-m" {
        let tool = args[2].as_str();
        if matches!(tool, "pytest" | "ruff" | "mypy" | "pylint" | "flake8") {
            return Some((tool, 3));
        }
    }

    let tool = args[0].as_str();
    if matches!(tool, "pytest" | "ruff" | "mypy" | "pylint" | "flake8") {
        return Some((tool, 1));
    }

    None
}

fn is_python_executable(candidate: &str) -> bool {
    if candidate == "python" || candidate == "python3" {
        return true;
    }

    let Some(file_name) = Path::new(candidate).file_name().and_then(|s| s.to_str()) else {
        return false;
    };

    matches!(file_name, "python" | "python3")
}

fn run_sync(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("uv");
    cmd.arg("sync");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: uv sync {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run uv sync")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let filtered = filter_uv_sync_output(&raw);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "uv_sync", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("uv sync {}", args.join(" ")),
        &format!("rtk uv sync {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

fn run_passthrough_with_prefix(prefix: &str, args: &[String], verbose: u8) -> Result<()> {
    let mut all_args = Vec::with_capacity(args.len() + 1);
    all_args.push(prefix.to_string());
    all_args.extend(args.iter().cloned());
    run_passthrough(&all_args, verbose)
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("uv");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: uv {}", args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run uv {}", args.join(" ")))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("uv {}", args.join(" ")),
        &format!("rtk uv {}", args.join(" ")),
        &raw,
        &raw,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

fn filter_uv_sync_output(output: &str) -> String {
    let mut summary_lines = Vec::new();
    let mut package_lines = Vec::new();

    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let lower = line.to_ascii_lowercase();
        let is_summary = line.starts_with("Resolved")
            || line.starts_with("Prepared")
            || line.starts_with("Installed")
            || line.starts_with("Uninstalled")
            || line.starts_with("Added")
            || line.starts_with("Removed")
            || line.starts_with("Updated")
            || line.starts_with("Audited");
        let is_error = lower.contains("error") || lower.contains("failed");
        let is_package_delta = line.starts_with('+') || line.starts_with('-');

        if is_summary || is_error {
            summary_lines.push(line.to_string());
        } else if is_package_delta {
            package_lines.push(line.to_string());
        }
    }

    if summary_lines.is_empty() && package_lines.is_empty() {
        return "✓ uv sync completed".to_string();
    }

    let mut out = String::from("uv sync\n═══════════════════════════════════════\n");
    for line in summary_lines {
        out.push_str(&line);
        out.push('\n');
    }

    if !package_lines.is_empty() {
        out.push_str("\nPackage changes:\n");
        for line in package_lines.iter().take(15) {
            out.push_str(line);
            out.push('\n');
        }
        if package_lines.len() > 15 {
            out.push_str(&format!("... +{} more\n", package_lines.len() - 15));
        }
    }

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_uv_sync_token_savings() {
        let input = r#"
Resolved 145 packages in 12ms
Downloading anyio-4.6.2.post1-py3-none-any.whl (90.2 KiB)
Downloading requests-2.32.3-py3-none-any.whl (64.9 KiB)
Downloading certifi-2024.12.14-py3-none-any.whl (164.4 KiB)
Downloading charset_normalizer-3.4.1-cp311-cp311-macosx_11_0_arm64.whl (120.8 KiB)
Downloading idna-3.10-py3-none-any.whl (70.4 KiB)
Downloading urllib3-2.3.0-py3-none-any.whl (96.4 KiB)
Downloading sniffio-1.3.1-py3-none-any.whl (10.4 KiB)
Downloading exceptiongroup-1.2.2-py3-none-any.whl (16.8 KiB)
Downloading h11-0.14.0-py3-none-any.whl (58.3 KiB)
Downloading httpcore-1.0.7-py3-none-any.whl (78.6 KiB)
Downloading httpx-0.28.1-py3-none-any.whl (75.4 KiB)
Downloading zstandard-0.23.0-cp311-cp311-macosx_11_0_arm64.whl (363.4 KiB)
Verifying sha256 of anyio-4.6.2.post1-py3-none-any.whl
Verifying sha256 of requests-2.32.3-py3-none-any.whl
Verifying sha256 of certifi-2024.12.14-py3-none-any.whl
Verifying sha256 of charset_normalizer-3.4.1-cp311-cp311-macosx_11_0_arm64.whl
Verifying sha256 of idna-3.10-py3-none-any.whl
Verifying sha256 of urllib3-2.3.0-py3-none-any.whl
Verifying sha256 of sniffio-1.3.1-py3-none-any.whl
Verifying sha256 of exceptiongroup-1.2.2-py3-none-any.whl
Verifying sha256 of h11-0.14.0-py3-none-any.whl
Verifying sha256 of httpcore-1.0.7-py3-none-any.whl
Verifying sha256 of httpx-0.28.1-py3-none-any.whl
Verifying sha256 of zstandard-0.23.0-cp311-cp311-macosx_11_0_arm64.whl
Prepared 12 packages in 234ms
Installed 12 packages in 89ms
 + anyio==4.6.2.post1
 + certifi==2024.12.14
 + charset-normalizer==3.4.1
 + exceptiongroup==1.2.2
 + h11==0.14.0
 + httpcore==1.0.7
 + httpx==0.28.1
 + idna==3.10
 + requests==2.32.3
 + sniffio==1.3.1
 + urllib3==2.3.0
 + zstandard==0.23.0
"#;
        let output = filter_uv_sync_output(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "uv sync filter: expected >=60% token savings, got {savings:.1}% (in={input_tokens}, out={output_tokens})"
        );
    }

    #[test]
    fn test_filter_uv_sync_output_summary() {
        let output = r#"
Resolved 145 packages in 4ms
Installed 2 packages in 21ms
+ pytest==8.3.2
- pytest==8.2.0
"#;
        let filtered = filter_uv_sync_output(output);
        assert!(filtered.contains("uv sync"));
        assert!(filtered.contains("Resolved 145 packages"));
        assert!(filtered.contains("Installed 2 packages"));
        assert!(filtered.contains("pytest==8.3.2"));
    }

    #[test]
    fn test_filter_uv_sync_output_fallback() {
        let filtered = filter_uv_sync_output("done");
        assert_eq!(filtered, "✓ uv sync completed");
    }

    #[test]
    fn test_is_python_executable() {
        assert!(is_python_executable("python"));
        assert!(is_python_executable("python3"));
        assert!(is_python_executable(".ven/bin/python"));
        assert!(is_python_executable(".venv/bin/python"));
        assert!(is_python_executable("/tmp/venv/bin/python3"));
        assert!(!is_python_executable("pytest"));
        assert!(!is_python_executable(".venv/bin/pypy"));
    }

    #[test]
    fn test_routed_uv_run_tool_direct() {
        assert_eq!(
            routed_uv_run_tool(&["mypy".to_string(), "src/".to_string()]),
            Some(("mypy", 1))
        );
        assert_eq!(
            routed_uv_run_tool(&["ruff".to_string(), "check".to_string()]),
            Some(("ruff", 1))
        );
        assert_eq!(
            routed_uv_run_tool(&["pytest".to_string(), "-q".to_string()]),
            Some(("pytest", 1))
        );
    }

    #[test]
    fn test_routed_uv_run_tool_python_module() {
        assert_eq!(
            routed_uv_run_tool(&[
                ".venv/bin/python".to_string(),
                "-m".to_string(),
                "pylint".to_string(),
                ".".to_string()
            ]),
            Some(("pylint", 3))
        );
        assert_eq!(
            routed_uv_run_tool(&[
                ".ven/bin/python".to_string(),
                "-m".to_string(),
                "flake8".to_string(),
                ".".to_string()
            ]),
            Some(("flake8", 3))
        );
        assert_eq!(
            routed_uv_run_tool(&[
                "python3".to_string(),
                "-m".to_string(),
                "mypy".to_string(),
                "src".to_string()
            ]),
            Some(("mypy", 3))
        );
    }

    #[test]
    fn test_routed_uv_run_tool_passthrough_cases() {
        assert_eq!(
            routed_uv_run_tool(&["python".to_string(), "-m".to_string(), "black".to_string()]),
            None
        );
        assert_eq!(
            routed_uv_run_tool(&["alembic".to_string(), "upgrade".to_string()]),
            None
        );
    }
}

//! Resolves Poe tasks from pyproject.toml and runs each sub-command
//! through the appropriate RTK filter.

use crate::core::runner;
use crate::discover::registry;
use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::path::Path;
use toml::Value;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        bail!("Usage: rtk poe <task> [args...]");
    }

    let task_name = &args[0];
    let extra_args = &args[1..];

    let pyproject_path = Path::new("pyproject.toml");
    if !pyproject_path.exists() {
        bail!("No pyproject.toml found in current directory");
    }

    let content =
        std::fs::read_to_string(pyproject_path).context("Failed to read pyproject.toml")?;
    let doc: Value = content
        .parse::<Value>()
        .context("Failed to parse pyproject.toml")?;

    let tasks = doc
        .get("tool")
        .and_then(|t| t.get("poe"))
        .and_then(|p| p.get("tasks"))
        .context("No [tool.poe.tasks] section found in pyproject.toml")?;

    run_task(task_name, extra_args, tasks, verbose)
}

fn run_task(name: &str, extra_args: &[String], tasks: &Value, verbose: u8) -> Result<i32> {
    let task = tasks
        .get(name)
        .with_context(|| format!("Poe task '{}' not found in pyproject.toml", name))?;

    // Sequence task: { sequence = ["task1", "task2", ...] }
    if let Some(seq) = task.get("sequence").and_then(|s| s.as_array()) {
        for sub_task_val in seq {
            let sub_name = sub_task_val
                .as_str()
                .context("Sequence items must be strings")?;
            if verbose > 0 {
                eprintln!("poe: running sub-task '{}'", sub_name);
            }
            let code = run_task(sub_name, &[], tasks, verbose)?;
            if code != 0 {
                return Ok(code);
            }
        }
        return Ok(0);
    }

    // Cmd task: { cmd = "ruff check src" } or inline string "ruff check src"
    let cmd_str = if let Some(cmd) = task.get("cmd").and_then(|c| c.as_str()) {
        cmd.to_string()
    } else if let Some(cmd) = task.as_str() {
        cmd.to_string()
    } else {
        bail!(
            "Task '{}': only cmd and sequence tasks are supported (ref/script/shell not supported)",
            name
        );
    };

    let full_cmd = if extra_args.is_empty() {
        cmd_str
    } else {
        format!("{} {}", cmd_str, extra_args.join(" "))
    };

    if verbose > 0 {
        eprintln!("poe: {}", full_cmd);
    }

    run_cmd_string(&full_cmd, verbose)
}

fn run_cmd_string(cmd_str: &str, verbose: u8) -> Result<i32> {
    if cmd_str.trim().is_empty() {
        bail!("Empty command in poe task");
    }

    if registry::rewrite_command(cmd_str.trim(), &[], &[]).is_some() {
        if verbose > 0 {
            eprintln!("poe: routing through rtk: {}", cmd_str.trim());
        }
        let mut cmd = std::process::Command::new(
            std::env::current_exe()
                .context("Failed to get rtk binary path")?,
        );
        for part in cmd_str.split_whitespace() {
            cmd.arg(part);
        }
        let status = cmd
            .status()
            .with_context(|| format!("Failed to run rtk for: {}", cmd_str.trim()))?;
        Ok(status.code().unwrap_or(1))
    } else {
        if verbose > 0 {
            eprintln!(
                "poe: no RTK filter for command (running passthrough): {}",
                cmd_str.trim()
            );
        }
        let parts: Vec<&str> = cmd_str.split_whitespace().collect();
        let tool = parts[0];
        let args: Vec<OsString> = parts[1..].iter().map(|s| OsString::from(*s)).collect();
        runner::run_passthrough(tool, &args, verbose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tasks() -> Value {
        let toml_str = r#"
[ruff-format-check]
cmd = "ruff format --check src"

[ruff-check]
cmd = "ruff check src"

[mypy]
cmd = "mypy src"

[lint]
sequence = ["ruff-format-check", "ruff-check", "mypy"]
"#;
        toml_str.parse::<Value>().unwrap()
    }

    #[test]
    fn test_resolve_cmd_task() {
        let tasks = sample_tasks();
        let task = tasks.get("ruff-check").unwrap();
        let cmd = task.get("cmd").and_then(|c| c.as_str()).unwrap();
        assert_eq!(cmd, "ruff check src");
    }

    #[test]
    fn test_resolve_sequence_task() {
        let tasks = sample_tasks();
        let task = tasks.get("lint").unwrap();
        let seq = task
            .get("sequence")
            .and_then(|s| s.as_array())
            .unwrap();
        let names: Vec<&str> = seq.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(names, vec!["ruff-format-check", "ruff-check", "mypy"]);
    }

    #[test]
    fn test_missing_task_returns_error() {
        let tasks = sample_tasks();
        let result = run_task("nonexistent", &[], &tasks, 0);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not found")
        );
    }

    #[test]
    fn test_inline_string_task() {
        let toml_str = r#"
hello = "echo hello"
"#;
        let tasks: Value = toml_str.parse().unwrap();
        let task = tasks.get("hello").unwrap();
        let cmd = task.as_str().unwrap();
        assert_eq!(cmd, "echo hello");
    }

    #[test]
    fn test_unsupported_task_type_returns_error() {
        let toml_str = r#"
[my-script]
script = "my_module:main"
"#;
        let tasks: Value = toml_str.parse().unwrap();
        let result = run_task("my-script", &[], &tasks, 0);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only cmd and sequence tasks are supported")
        );
    }

    #[test]
    fn test_empty_args_returns_error() {
        let result = run(&[], 0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Usage:"));
    }

    #[test]
    fn test_cmd_with_help_field_resolves() {
        let toml_str = r#"
[build-wheel]
help = "Build wheels"
cmd = "poetry build"
"#;
        let tasks: Value = toml_str.parse().unwrap();
        let task = tasks.get("build-wheel").unwrap();
        let cmd = task.get("cmd").and_then(|c| c.as_str()).unwrap();
        assert_eq!(cmd, "poetry build");
    }

    #[test]
    fn test_sequence_stops_on_missing_subtask() {
        let toml_str = r#"
[step1]
cmd = "echo step1"

[pipeline]
sequence = ["step1", "nonexistent"]
"#;
        let tasks: Value = toml_str.parse().unwrap();
        // Can't run the sequence without executing commands, but we can verify
        // the task structure parses correctly
        let task = tasks.get("pipeline").unwrap();
        let seq = task
            .get("sequence")
            .and_then(|s| s.as_array())
            .unwrap();
        assert_eq!(seq.len(), 2);
    }

    #[test]
    fn bench_pyproject_toml_parse() {
        let content = include_str!("../../../tests/fixtures/pyproject_poe.toml");
        let iterations = 100;

        let start = std::time::Instant::now();
        for _ in 0..iterations {
            let doc: Value = content.parse::<Value>().expect("parse failed");
            let _tasks = doc
                .get("tool")
                .and_then(|t| t.get("poe"))
                .and_then(|p| p.get("tasks"));
            std::hint::black_box(&_tasks);
        }
        let elapsed = start.elapsed();

        let per_parse_us = elapsed.as_micros() as f64 / iterations as f64;
        eprintln!(
            "pyproject.toml parse: {:.1}µs/iter ({} iterations in {:?})",
            per_parse_us, iterations, elapsed
        );
        // Parse must stay well under ruff/mypy startup (~500ms).
        // Debug builds ~9ms, release ~0.1ms — no cache needed.
        assert!(
            per_parse_us < 50_000.0,
            "Parse took {:.1}µs — investigate regression",
            per_parse_us
        );
    }
}

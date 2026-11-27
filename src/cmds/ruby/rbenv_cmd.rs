//! Routes `rbenv exec <ruby-tool>` through RTK's Ruby filters while executing
//! the caller's exact `rbenv exec ...` command.
//!
//! `rbenv exec` resolves its target via `rbenv-which`, which only searches
//! rbenv's managed Ruby version bin directory rather than `PATH`. Placing `rtk`
//! under `rbenv exec` (`rbenv exec rtk ...`) therefore fails with
//! `rbenv: rtk: command not found`, while dropping `rbenv exec` would bypass
//! the requested Ruby version. Keeping `rtk` outside (`rtk rbenv exec ...`)
//! runs `rbenv exec` inside RTK and filters the inner Ruby tool's output.

use super::{rake_cmd, rspec_cmd, rubocop_cmd};
use crate::core::runner;
use crate::core::shell;
use crate::core::tee;
use crate::core::toml_filter;
use crate::core::tracking;
use crate::core::utils::{
    ChildArgExt, decode_process_output, exit_code_from_output, resolved_command,
};
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::process::{Command, Stdio};

#[derive(Debug, PartialEq, Eq)]
enum RbenvTarget<'a> {
    Rspec {
        uses_bundle: bool,
        tool: &'a str,
        rest: &'a [String],
    },
    Rubocop {
        uses_bundle: bool,
        tool: &'a str,
        rest: &'a [String],
    },
    Rake {
        uses_bundle: bool,
        tool: &'a str,
        rest: &'a [String],
    },
    BundleToml {
        exec_args: &'a [String],
    },
    Passthrough,
}

fn is_ruby_test_task(args: &[String]) -> bool {
    args.first()
        .is_some_and(|a| a == "test" || a.starts_with("test:"))
}

fn classify_rbenv_args(args: &[String]) -> RbenvTarget<'_> {
    let [first, exec_args @ ..] = args else {
        return RbenvTarget::Passthrough;
    };
    if first != "exec" || exec_args.is_empty() {
        return RbenvTarget::Passthrough;
    }

    let (uses_bundle, tool_and_args) = match exec_args {
        [b, e, rest @ ..] if b == "bundle" && e == "exec" && !rest.is_empty() => (true, rest),
        _ => (false, exec_args),
    };

    match tool_and_args {
        [tool, rest @ ..] if matches!(tool.as_str(), "rspec" | "bin/rspec") => RbenvTarget::Rspec {
            uses_bundle,
            tool: tool.as_str(),
            rest,
        },
        [tool, rest @ ..] if tool == "rubocop" => RbenvTarget::Rubocop {
            uses_bundle,
            tool: tool.as_str(),
            rest,
        },
        [tool, rest @ ..]
            if matches!(tool.as_str(), "rake" | "rails" | "bin/rake" | "bin/rails")
                && is_ruby_test_task(rest) =>
        {
            RbenvTarget::Rake {
                uses_bundle,
                tool: tool.as_str(),
                rest,
            }
        }
        [b, sub, ..]
            if !uses_bundle && b == "bundle" && matches!(sub.as_str(), "install" | "update") =>
        {
            RbenvTarget::BundleToml { exec_args }
        }
        _ => RbenvTarget::Passthrough,
    }
}

fn build_rbenv_command(uses_bundle: bool, tool: &str) -> (Command, String) {
    let mut cmd = resolved_command("rbenv");
    cmd.arg("exec");
    let label = if uses_bundle {
        cmd.arg("bundle").arg("exec");
        format!("rbenv exec bundle exec {tool}")
    } else {
        format!("rbenv exec {tool}")
    };
    cmd.arg(tool);
    (cmd, label)
}

fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    runner::run_passthrough("rbenv", &os_args, verbose)
}

fn run_bundle_toml(args: &[String], exec_args: &[String], verbose: u8) -> Result<i32> {
    if toml_filter::toml_disabled() {
        return run_passthrough(args, verbose);
    }

    let lookup_cmd = exec_args.join(" ");
    let Some(filter) = toml_filter::find_matching_filter(&lookup_cmd) else {
        return run_passthrough(args, verbose);
    };

    if verbose > 0 {
        eprintln!("Running: rbenv {}", args.join(" "));
    }

    let timer = tracking::TimedExecution::start();
    let raw_command = format!("rbenv {}", args.join(" "));
    let tracked_command = shell::with_args("rbenv", &shell::display_args(args));

    let mut cmd = resolved_command("rbenv");
    cmd.child_args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(if filter.filter_stderr {
            Stdio::piped()
        } else {
            Stdio::inherit()
        });

    let output = cmd.output().context("Failed to run rbenv")?;
    let exit_code = exit_code_from_output(&output, &raw_command);
    let stdout_raw = decode_process_output(&output.stdout);
    let stderr_raw = decode_process_output(&output.stderr);
    let combined_raw = if filter.filter_stderr {
        format!("{}{}", stdout_raw, stderr_raw)
    } else {
        stdout_raw.to_string()
    };

    let (filtered, loss) = toml_filter::apply_filter_with_info(filter, &combined_raw);
    let lossy = !matches!(loss, toml_filter::Lossiness::None);

    let hint = if !output.status.success() {
        tee::tee_and_hint(&combined_raw, &raw_command, exit_code)
    } else {
        match &loss {
            toml_filter::Lossiness::None => None,
            toml_filter::Lossiness::Tail {
                tee_payload,
                tail_offset,
            } => tee::force_tee_tail_hint(tee_payload, &raw_command, *tail_offset),
            toml_filter::Lossiness::Whole => tee::force_tee_hint(&combined_raw, &raw_command),
        }
    };

    let shown = if lossy && hint.is_none() {
        runner::emit_guarded(&combined_raw, None, &combined_raw)
    } else {
        runner::emit_guarded(&filtered, hint.as_deref(), &combined_raw)
    };

    timer.track(
        &tracked_command,
        &format!("rtk:toml {}", tracked_command),
        &combined_raw,
        &shown,
    );

    Ok(exit_code)
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if runner::requests_help_args("rbenv", args) {
        return run_passthrough(args, verbose);
    }

    match classify_rbenv_args(args) {
        RbenvTarget::Rspec {
            uses_bundle,
            tool,
            rest,
        } => {
            let (cmd, label) = build_rbenv_command(uses_bundle, tool);
            rspec_cmd::run_with_command(cmd, &label, rest, verbose)
        }
        RbenvTarget::Rubocop {
            uses_bundle,
            tool,
            rest,
        } => {
            let (cmd, label) = build_rbenv_command(uses_bundle, tool);
            rubocop_cmd::run_with_command(cmd, &label, rest, verbose)
        }
        RbenvTarget::Rake {
            uses_bundle,
            tool,
            rest,
        } => {
            let (cmd, label) = build_rbenv_command(uses_bundle, tool);
            rake_cmd::run_with_command(cmd, &label, rest, verbose)
        }
        RbenvTarget::BundleToml { exec_args } => run_bundle_toml(args, exec_args, verbose),
        RbenvTarget::Passthrough => run_passthrough(args, verbose),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn test_classify_rbenv_exec_ruby_tools() {
        let rspec = vec_s(&["exec", "bundle", "exec", "rspec", "spec/foo_spec.rb"]);
        assert_eq!(
            classify_rbenv_args(&rspec),
            RbenvTarget::Rspec {
                uses_bundle: true,
                tool: "rspec",
                rest: &rspec[4..],
            }
        );

        let direct_rspec = vec_s(&["exec", "rspec", "spec/foo_spec.rb"]);
        assert_eq!(
            classify_rbenv_args(&direct_rspec),
            RbenvTarget::Rspec {
                uses_bundle: false,
                tool: "rspec",
                rest: &direct_rspec[2..],
            }
        );

        let rubocop = vec_s(&["exec", "bundle", "exec", "rubocop", "app/"]);
        assert_eq!(
            classify_rbenv_args(&rubocop),
            RbenvTarget::Rubocop {
                uses_bundle: true,
                tool: "rubocop",
                rest: &rubocop[4..],
            }
        );

        let rails_test = vec_s(&["exec", "bundle", "exec", "rails", "test", "test/a_test.rb"]);
        assert_eq!(
            classify_rbenv_args(&rails_test),
            RbenvTarget::Rake {
                uses_bundle: true,
                tool: "rails",
                rest: &rails_test[4..],
            }
        );

        let rake_namespaced = vec_s(&["exec", "rake", "test:models"]);
        assert_eq!(
            classify_rbenv_args(&rake_namespaced),
            RbenvTarget::Rake {
                uses_bundle: false,
                tool: "rake",
                rest: &rake_namespaced[2..],
            }
        );

        let bundle_install = vec_s(&["exec", "bundle", "install", "--jobs=4"]);
        assert_eq!(
            classify_rbenv_args(&bundle_install),
            RbenvTarget::BundleToml {
                exec_args: &bundle_install[1..],
            }
        );
    }

    #[test]
    fn test_classify_rbenv_non_target_commands_passthrough() {
        for case in [
            vec_s(&[]),
            vec_s(&["versions"]),
            vec_s(&["exec"]),
            vec_s(&["exec", "ruby", "-e", "puts 1"]),
            vec_s(&["exec", "rails", "runner", "User.count"]),
            vec_s(&["exec", "bundle", "exec", "rails", "console"]),
            vec_s(&["exec", "bundle", "exec", "bundle", "install"]),
            vec_s(&["exec", "rake", "db:migrate"]),
            vec_s(&["exec", "rake", "tester"]),
        ] {
            assert_eq!(
                classify_rbenv_args(&case),
                RbenvTarget::Passthrough,
                "expected passthrough for {case:?}"
            );
        }
    }

    #[test]
    fn test_build_rbenv_command_preserves_caller_wrapper_and_tool() {
        let (cmd, label) = build_rbenv_command(true, "rspec");
        let argv: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["exec", "bundle", "exec", "rspec"]);
        assert_eq!(label, "rbenv exec bundle exec rspec");

        let (cmd, label) = build_rbenv_command(false, "bin/rspec");
        let argv: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(argv, vec!["exec", "bin/rspec"]);
        assert_eq!(label, "rbenv exec bin/rspec");
    }
}

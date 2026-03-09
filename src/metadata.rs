use clap::CommandFactory;
use std::ffi::OsString;

use crate::Cli;

#[derive(Clone, Copy)]
pub(crate) struct TopLevelCommandMetadata {
    pub(crate) name: &'static str,
    pub(crate) operational: bool,
    pub(crate) shim: bool,
    pub(crate) metadata: bool,
}

pub(crate) const TOP_LEVEL_COMMAND_METADATA: &[TopLevelCommandMetadata] = &[
    TopLevelCommandMetadata {
        name: "aws",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "cargo",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "cc-economics",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "config",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "curl",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "deps",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "diff",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "discover",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "docker",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "env",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "err",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "find",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "format",
        operational: false,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "gain",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "gh",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "git",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "go",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "golangci-lint",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "grep",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "gt",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "hook-audit",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "init",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "json",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "kubectl",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "learn",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "lint",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "log",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "ls",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "mypy",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "next",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "npm",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "npx",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "pip",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "playwright",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "pnpm",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "prettier",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "prisma",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "proxy",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "psql",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "pytest",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "read",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "rewrite",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "ruff",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "shim",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "smart",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "summary",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "test",
        operational: true,
        shim: false,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "tree",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "tsc",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "verify",
        operational: false,
        shim: false,
        metadata: true,
    },
    TopLevelCommandMetadata {
        name: "vitest",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "wc",
        operational: true,
        shim: true,
        metadata: false,
    },
    TopLevelCommandMetadata {
        name: "wget",
        operational: true,
        shim: true,
        metadata: false,
    },
];

pub(crate) fn is_rtk_meta_command(name: &str) -> bool {
    top_level_command_metadata(name)
        .map(|meta| meta.metadata)
        .unwrap_or(false)
}

pub(crate) fn supported_top_level_commands() -> Vec<String> {
    let mut names: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|sub| sub.get_name().to_string())
        .collect();
    names.sort();
    names
}

pub(crate) fn is_supported_top_level_command(name: &str) -> bool {
    Cli::command()
        .get_subcommands()
        .any(|sub| sub.get_name() == name)
}

pub(crate) fn top_level_command_metadata(name: &str) -> Option<&'static TopLevelCommandMetadata> {
    TOP_LEVEL_COMMAND_METADATA
        .iter()
        .find(|meta| meta.name == name)
}

pub(crate) fn is_operational_command_from_parse_argv(parse_argv: &[OsString]) -> bool {
    let Ok(matches) = Cli::command().try_get_matches_from(parse_argv.iter().cloned()) else {
        return false;
    };
    let Some(command_name) = matches.subcommand_name() else {
        return false;
    };

    top_level_command_metadata(command_name)
        .map(|meta| meta.operational)
        .unwrap_or(false)
}

pub(crate) fn shim_eligible_top_level_commands() -> Vec<String> {
    let mut names: Vec<String> = supported_top_level_commands()
        .into_iter()
        .filter(|name| {
            top_level_command_metadata(name)
                .map(|meta| meta.shim)
                .unwrap_or(false)
        })
        .collect();
    names.sort();
    names
}

pub(crate) fn is_shim_eligible_top_level_command(name: &str) -> bool {
    is_supported_top_level_command(name)
        && top_level_command_metadata(name)
            .map(|meta| meta.shim)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn os_argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn test_supported_top_level_commands_contains_known_items() {
        let commands = supported_top_level_commands();
        assert!(commands.iter().any(|c| c == "git"));
        assert!(commands.iter().any(|c| c == "curl"));
        assert!(commands.iter().any(|c| c == "shim"));
    }

    #[test]
    fn test_top_level_command_metadata_covers_supported_commands() {
        for cmd in supported_top_level_commands() {
            assert!(
                top_level_command_metadata(&cmd).is_some(),
                "Missing top-level metadata for command '{}'",
                cmd
            );
        }
    }

    #[test]
    fn test_top_level_command_metadata_has_no_unknown_commands() {
        for meta in TOP_LEVEL_COMMAND_METADATA {
            assert!(
                is_supported_top_level_command(meta.name),
                "Unknown command '{}' exists in top-level metadata",
                meta.name
            );
        }
    }

    #[test]
    fn test_top_level_command_metadata_commands_match_expected_set() {
        let actual: std::collections::BTreeSet<&str> = TOP_LEVEL_COMMAND_METADATA
            .iter()
            .filter(|meta| meta.metadata)
            .map(|meta| meta.name)
            .collect();

        let expected: std::collections::BTreeSet<&str> = [
            "cc-economics",
            "config",
            "discover",
            "gain",
            "hook-audit",
            "init",
            "learn",
            "proxy",
            "rewrite",
            "shim",
            "verify",
        ]
        .into_iter()
        .collect();

        assert_eq!(actual, expected, "metadata command set drifted");
    }

    #[test]
    fn test_meta_commands_reject_bad_flags() {
        // RTK-native commands should produce parse errors (not fall through to raw execution).
        // Skip "proxy" because it uses trailing_var_arg (accepts any args by design).
        let guarded = [
            "gain",
            "discover",
            "learn",
            "init",
            "config",
            "shim",
            "hook-audit",
            "cc-economics",
            "verify",
        ];
        for cmd in guarded {
            let result = Cli::try_parse_from(["rtk", cmd, "--nonexistent-flag-xyz"]);
            assert!(
                result.is_err(),
                "Guarded command '{}' with bad flag should fail to parse",
                cmd
            );
        }
    }

    #[test]
    fn test_guarded_command_list_parses_valid_invocations() {
        let guarded_cmds_that_parse = [
            vec!["rtk", "gain"],
            vec!["rtk", "discover"],
            vec!["rtk", "learn"],
            vec!["rtk", "init"],
            vec!["rtk", "config"],
            vec!["rtk", "shim", "install", "git"],
            vec!["rtk", "proxy", "echo", "hi"],
            vec!["rtk", "hook-audit"],
            vec!["rtk", "cc-economics"],
            vec!["rtk", "verify"],
            vec!["rtk", "rewrite", "git status"],
        ];
        for args in &guarded_cmds_that_parse {
            let result = Cli::try_parse_from(args.iter());
            assert!(
                result.is_ok(),
                "Guarded command {:?} should parse successfully",
                args
            );
        }
    }

    #[test]
    fn test_git_is_operational() {
        assert!(is_operational_command_from_parse_argv(&os_argv(&[
            "rtk", "git", "status",
        ])));
    }

    #[test]
    fn test_aws_psql_wc_mypy_are_operational() {
        assert!(is_operational_command_from_parse_argv(&os_argv(&[
            "rtk", "aws", "sts",
        ])));
        assert!(is_operational_command_from_parse_argv(&os_argv(&[
            "rtk", "psql",
        ])));
        assert!(is_operational_command_from_parse_argv(&os_argv(&[
            "rtk", "wc",
        ])));
        assert!(is_operational_command_from_parse_argv(&os_argv(&[
            "rtk", "mypy",
        ])));
    }
}

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

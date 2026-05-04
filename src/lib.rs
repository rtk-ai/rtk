//! RTK (Rust Token Killer) library crate.
//!
//! Re-exports all public modules for use by downstream crates (e.g. rtk-pro).
//! The binary crate (`main.rs`) uses its own `mod` declarations to compile the
//! same source files independently.

pub mod analytics;
pub mod cmds;
pub mod core;
pub mod discover;
pub mod dispatch;
pub mod hooks;
pub mod learn;
pub mod parser;

// Re-export command modules (mirrors main.rs use statements)
pub use cmds::cloud::{aws_cmd, container, curl_cmd, psql_cmd, wget_cmd};
pub use cmds::dotnet::{binlog, dotnet_cmd, dotnet_format_report, dotnet_trx};
pub use cmds::git::{diff_cmd, gh_cmd, git, glab_cmd, gt_cmd};
pub use cmds::go::{go_cmd, golangci_cmd};
pub use cmds::js::{
    lint_cmd, next_cmd, npm_cmd, playwright_cmd, pnpm_cmd, prettier_cmd, prisma_cmd, tsc_cmd,
    vitest_cmd,
};
pub use cmds::python::{mypy_cmd, pip_cmd, pytest_cmd, ruff_cmd};
pub use cmds::ruby::{rake_cmd, rspec_cmd, rubocop_cmd};
pub use cmds::rust::cargo_cmd;
pub use cmds::rust::runner as cargo_runner;
pub use cmds::system::{
    deps, env_cmd, find_cmd, format_cmd, grep_cmd, json_cmd, local_llm, log_cmd, ls, pipe_cmd,
    read, summary, tree, wc_cmd,
};

// Re-export core modules
pub use self::core::{
    config, constants, display_helpers, filter, runner, stream, tee, telemetry, telemetry_cmd,
    toml_filter, tracking, utils,
};

// Re-export analytics
pub use analytics::{cc_economics, ccusage, gain, session_cmd};

// Re-export hooks
pub use hooks::{
    hook_audit_cmd, hook_check, hook_cmd, init, integrity, permissions, rewrite_cmd, trust,
    verify_cmd,
};

// Re-export dispatch entry point for downstream crates (rtk-pro)
pub use dispatch::oss_command_names;
pub use dispatch::run_from_args;

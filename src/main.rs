mod analytics;
mod cmds;
mod core;
mod discover;
mod hooks;
mod learn;
mod parser;

// Re-export command modules for routing
use cmds::cloud::{aws_cmd, container, curl_cmd, psql_cmd, wget_cmd};
use cmds::dotnet::{binlog, dotnet_cmd, dotnet_format_report, dotnet_trx};
use cmds::git::{diff_cmd, gh_cmd, git, gt_cmd};
use cmds::go::{go_cmd, golangci_cmd};
use cmds::js::{
    lint_cmd, next_cmd, npm_cmd, playwright_cmd, pnpm_cmd, prettier_cmd, prisma_cmd, tsc_cmd,
    vitest_cmd,
};
use cmds::python::{mypy_cmd, pip_cmd, pytest_cmd, ruff_cmd};
use cmds::ruby::{rake_cmd, rspec_cmd, rubocop_cmd};
use cmds::rust::{cargo_cmd, runner};
use cmds::system::{
    deps, env_cmd, find_cmd, format_cmd, grep_cmd, json_cmd, local_llm, log_cmd, ls, read, summary,
    tree, wc_cmd,
};

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Target agent for hook installation.
#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum AgentTarget {
    /// Claude Code (default)
    Claude,
    /// Cursor Agent (editor and CLI)
    Cursor,
    /// Windsurf IDE (Cascade)
    Windsurf,
    /// Cline / Roo Code (VS Code)
    Cline,
    /// Kilo Code
    Kilocode,
    /// Google Antigravity
    Antigravity,
}

const HELP_TEMPLATE: &str = "\
{name} {version}
{about-with-newline}
{usage-heading} {usage}

RTK:
  cc-economics   Spending vs savings analysis
  config         Show/create configuration
  discover       Find missed savings opportunities
  gain           Token savings summary and history
  hook           Hook processors for AI tools
  hook-audit     Hook rewrite audit metrics
  init           Initialize rtk for your AI agent
  learn          Learn from CLI error history
  proxy          Execute without filtering (track only)
  rewrite        Rewrite command to RTK equivalent
  session        RTK adoption across sessions
  telemetry      Manage telemetry consent (GDPR)
  trust          Trust project TOML filters
  untrust        Revoke TOML filter trust
  verify         Verify hook + TOML filters

─────────────────────────────────────────────────────────

Analysis:
  deps           Summarize project dependencies
  env            Environment variables (masked)
  err            Run command, show errors only
  json           JSON compact or schema-only
  log            Filter and deduplicate logs
  smart          2-line technical summary
  summary        Run command, show summary

Build & Lint:
  cargo          Cargo (Rust)
  dotnet         .NET (build/test/restore/format)
  format         Universal format checker
  go             Go commands
  golangci-lint  golangci-lint grouped output
  lint           ESLint grouped violations
  mypy           Mypy type checker grouped
  prettier       Prettier format checker
  rubocop        RuboCop linter (Ruby)
  ruff           Ruff linter/formatter (Python)
  tsc            TypeScript compiler errors grouped

Cloud & Infrastructure:
  aws            AWS CLI (force JSON, compress)
  curl           Curl with auto-JSON detection
  docker         Docker compact output
  kubectl        Kubectl compact output
  psql           PostgreSQL (strip borders)
  wget           Download, strip progress bars

Files & Search:
  diff           Ultra-condensed diff
  find           Find files with compact output
  grep           Compact grep, grouped by file
  ls             List directory contents
  read           Read file with filtering
  tree           Directory tree
  wc             Compact word/line/byte count

Frameworks:
  next           Next.js build compact
  prisma         Prisma CLI (no ASCII art)

Git & GitHub:
  gh             GitHub CLI (pr, issue, run)
  git            Git with compact output
  gt             Graphite stacked PR commands

Package Managers:
  npm            npm with filtered output
  npx            npx with intelligent routing
  pip            pip/uv compact output
  pnpm           pnpm ultra-compact output

Test:
  playwright     Playwright E2E, failures only
  pytest         Pytest compact output
  rake           Rake/Rails Minitest compact
  rspec          RSpec (Rails/Ruby) compact
  test           Generic test runner, failures only
  vitest         Vitest compact output

Options:
{options}";

#[derive(Parser)]
#[command(
    name = "rtk",
    version,
    about = "Rust Token Killer - Minimize LLM token consumption",
    long_about = "A high-performance CLI proxy designed to filter and summarize system outputs before they reach your LLM context.",
    help_template = HELP_TEMPLATE,
    next_display_order = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Verbosity level (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Ultra-compact mode: ASCII icons, inline format (Level 2 optimizations)
    #[arg(long, global = true)]
    ultra_compact: bool,

    /// Set SKIP_ENV_VALIDATION=1 for child processes (Next.js, tsc, lint, prisma)
    #[arg(long = "skip-env", global = true)]
    skip_env: bool,
}

#[derive(Subcommand)]
enum Commands {
    // ── RTK ──────────────────────────────────────────────────────────────────
    /// Claude Code economics: spending (ccusage) vs savings (rtk) analysis
    #[command(next_display_order = None)]
    CcEconomics {
        /// Show detailed daily breakdown
        #[arg(short, long)]
        daily: bool,
        /// Show weekly breakdown
        #[arg(short, long)]
        weekly: bool,
        /// Show monthly breakdown
        #[arg(short, long)]
        monthly: bool,
        /// Show all time breakdowns (daily + weekly + monthly)
        #[arg(short, long)]
        all: bool,
        /// Output format: text, json, csv
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show or create configuration file
    #[command(next_display_order = None)]
    Config {
        /// Create default config file
        #[arg(long)]
        create: bool,
    },

    /// Discover missed RTK savings from Claude Code history
    #[command(next_display_order = None)]
    Discover {
        /// Filter by project path (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Max commands per section
        #[arg(short, long, default_value = "15")]
        limit: usize,
        /// Scan all projects (default: current project only)
        #[arg(short, long)]
        all: bool,
        /// Limit to sessions from last N days
        #[arg(short, long, default_value = "30")]
        since: u64,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show token savings summary and history
    #[command(next_display_order = None)]
    Gain {
        /// Filter statistics to current project (current working directory)
        #[arg(short, long)]
        project: bool,
        /// Show ASCII graph of daily savings
        #[arg(short, long)]
        graph: bool,
        /// Show recent command history
        #[arg(short = 'H', long)]
        history: bool,
        /// Show monthly quota savings estimate
        #[arg(short, long)]
        quota: bool,
        /// Subscription tier for quota calculation: pro, 5x, 20x
        #[arg(short, long, default_value = "20x", requires = "quota")]
        tier: String,
        /// Show detailed daily breakdown (all days)
        #[arg(short, long)]
        daily: bool,
        /// Show weekly breakdown
        #[arg(short, long)]
        weekly: bool,
        /// Show monthly breakdown
        #[arg(short, long)]
        monthly: bool,
        /// Show all time breakdowns (daily + weekly + monthly)
        #[arg(short, long)]
        all: bool,
        /// Output format: text, json, csv
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Show parse failure log (commands that fell back to raw execution)
        #[arg(short = 'F', long)]
        failures: bool,
    },

    /// Hook processors for LLM CLI tools (Gemini CLI, Copilot, etc.)
    #[command(next_display_order = None)]
    Hook {
        #[command(subcommand)]
        command: HookCommands,
    },

    /// Show hook rewrite audit metrics (requires RTK_HOOK_AUDIT=1)
    #[command(name = "hook-audit")]
    #[command(next_display_order = None)]
    HookAudit {
        /// Show entries from last N days (0 = all time)
        #[arg(short, long, default_value = "7")]
        since: u64,
    },

    /// Initialize rtk instructions for assistant CLI usage
    #[command(next_display_order = None)]
    Init {
        /// Add to global assistant config directory instead of local project file
        #[arg(short, long)]
        global: bool,

        /// Install OpenCode plugin (in addition to Claude Code)
        #[arg(long)]
        opencode: bool,

        /// Initialize for Gemini CLI instead of Claude Code
        #[arg(long)]
        gemini: bool,

        /// Target agent to install hooks for (default: claude)
        #[arg(long, value_enum)]
        agent: Option<AgentTarget>,

        /// Show current configuration
        #[arg(long)]
        show: bool,

        /// Inject full instructions into CLAUDE.md (legacy mode)
        #[arg(long = "claude-md", group = "mode")]
        claude_md: bool,

        /// Hook only, no RTK.md
        #[arg(long = "hook-only", group = "mode")]
        hook_only: bool,

        /// Auto-patch settings.json without prompting
        #[arg(long = "auto-patch", group = "patch")]
        auto_patch: bool,

        /// Skip settings.json patching (print manual instructions)
        #[arg(long = "no-patch", group = "patch")]
        no_patch: bool,

        /// Remove RTK artifacts for the selected assistant mode
        #[arg(long)]
        uninstall: bool,

        /// Target Codex CLI (uses AGENTS.md + RTK.md, no Claude hook patching)
        #[arg(long)]
        codex: bool,

        /// Install GitHub Copilot integration (VS Code + CLI)
        #[arg(long)]
        copilot: bool,
    },

    /// Learn CLI corrections from Claude Code error history
    #[command(next_display_order = None)]
    Learn {
        /// Filter by project path (substring match)
        #[arg(short, long)]
        project: Option<String>,
        /// Scan all projects (default: current project only)
        #[arg(short, long)]
        all: bool,
        /// Limit to sessions from last N days
        #[arg(short, long, default_value = "30")]
        since: u64,
        /// Output format: text, json
        #[arg(short, long, default_value = "text")]
        format: String,
        /// Generate .claude/rules/cli-corrections.md file
        #[arg(short, long)]
        write_rules: bool,
        /// Minimum confidence threshold (0.0-1.0)
        #[arg(long, default_value = "0.6")]
        min_confidence: f64,
        /// Minimum occurrences to include in report
        #[arg(long, default_value = "1")]
        min_occurrences: usize,
    },

    /// Execute command without filtering but track usage
    #[command(next_display_order = None)]
    Proxy {
        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },

    /// Rewrite a raw command to its RTK equivalent (single source of truth for hooks)
    ///
    /// Exits 0 and prints the rewritten command if supported.
    /// Exits 1 with no output if the command has no RTK equivalent.
    ///
    /// Used by Claude Code, Gemini CLI, and other LLM hooks:
    ///   REWRITTEN=$(rtk rewrite "$CMD") || exit 0
    #[command(next_display_order = None)]
    Rewrite {
        /// Raw command to rewrite (e.g. "git status", "cargo test && git push")
        /// Accepts multiple args: `rtk rewrite ls -al` is equivalent to `rtk rewrite "ls -al"`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Show RTK adoption across Claude Code sessions
    #[command(next_display_order = None)]
    Session {},

    /// Manage telemetry consent and data (RGPD/GDPR)
    Telemetry {
        #[command(subcommand)]
        command: core::telemetry_cmd::TelemetrySubcommand,
    },

    /// Trust project-local TOML filters in current directory
    #[command(next_display_order = None)]
    Trust {
        /// List all trusted projects
        #[arg(long)]
        list: bool,
    },

    /// Revoke trust for project-local TOML filters
    #[command(next_display_order = None)]
    Untrust,

    /// Verify hook integrity and run TOML filter inline tests
    #[command(next_display_order = None)]
    Verify {
        /// Run tests only for this filter name
        #[arg(long)]
        filter: Option<String>,
        /// Fail if any filter has no inline tests (CI mode)
        #[arg(long)]
        require_all: bool,
    },

    // ── Analysis ─────────────────────────────────────────────────────────────
    /// Summarize project dependencies
    #[command(next_display_order = None)]
    Deps {
        /// Project path
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Show environment variables (filtered, sensitive masked)
    #[command(next_display_order = None)]
    Env {
        /// Filter by name (e.g. PATH, AWS)
        #[arg(short, long)]
        filter: Option<String>,
        /// Show all (include sensitive)
        #[arg(long)]
        show_all: bool,
    },

    /// Run command and show only errors/warnings
    #[command(next_display_order = None)]
    Err {
        /// Command to run
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Show JSON (compact values by default, or keys-only with --keys-only)
    #[command(next_display_order = None)]
    Json {
        /// JSON file
        file: PathBuf,
        /// Max depth
        #[arg(short, long, default_value = "5")]
        depth: usize,
        /// Show keys only (strip all values, show structure)
        #[arg(long)]
        keys_only: bool,
    },

    /// Filter and deduplicate log output
    #[command(next_display_order = None)]
    Log {
        /// Log file (omit for stdin)
        file: Option<PathBuf>,
    },

    /// Generate 2-line technical summary (heuristic-based)
    #[command(next_display_order = None)]
    Smart {
        /// File to analyze
        file: PathBuf,
        /// Model: heuristic
        #[arg(short, long, default_value = "heuristic")]
        model: String,
        /// Force model download
        #[arg(long)]
        force_download: bool,
    },

    /// Run command and show heuristic summary
    #[command(next_display_order = None)]
    Summary {
        /// Command to run and summarize
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    // ── Build & Lint ──────────────────────────────────────────────────────────
    /// Cargo commands with compact output
    #[command(next_display_order = None)]
    Cargo {
        #[command(subcommand)]
        command: CargoCommands,
    },

    /// .NET commands with compact output (build/test/restore/format)
    #[command(next_display_order = None)]
    Dotnet {
        #[command(subcommand)]
        command: DotnetCommands,
    },

    /// Universal format checker (prettier, black, ruff format)
    #[command(next_display_order = None)]
    Format {
        /// Formatter arguments (auto-detects formatter from project files)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Go commands with compact output
    #[command(next_display_order = None)]
    Go {
        #[command(subcommand)]
        command: GoCommands,
    },

    /// golangci-lint wrapper with compact `run` support and passthrough for other invocations
    #[command(name = "golangci-lint")]
    #[command(next_display_order = None)]
    GolangciLint {
        /// Additional golangci-lint arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// ESLint with grouped rule violations
    #[command(next_display_order = None)]
    Lint {
        /// Linter arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Mypy type checker with grouped error output
    #[command(next_display_order = None)]
    Mypy {
        /// Mypy arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Prettier format checker with compact output
    #[command(next_display_order = None)]
    Prettier {
        /// Prettier arguments (e.g., --check, --write)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// RuboCop linter with compact output (Ruby)
    #[command(next_display_order = None)]
    Rubocop {
        /// RuboCop arguments (e.g., --auto-correct, -A)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Ruff linter/formatter with compact output
    #[command(next_display_order = None)]
    Ruff {
        /// Ruff arguments (e.g., check, format --check)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// TypeScript compiler with grouped error output
    #[command(next_display_order = None)]
    Tsc {
        /// TypeScript compiler arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // ── Cloud & Infrastructure ────────────────────────────────────────────────
    /// AWS CLI with compact output (force JSON, compress)
    #[command(next_display_order = None)]
    Aws {
        /// AWS service subcommand (e.g., sts, s3, ec2, ecs, rds, cloudformation)
        subcommand: String,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Curl with auto-JSON detection and schema output
    #[command(next_display_order = None)]
    Curl {
        /// Curl arguments (URL + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Docker commands with compact output
    #[command(next_display_order = None)]
    Docker {
        #[command(subcommand)]
        command: DockerCommands,
    },

    /// Kubectl commands with compact output
    #[command(next_display_order = None)]
    Kubectl {
        #[command(subcommand)]
        command: KubectlCommands,
    },

    /// PostgreSQL client with compact output (strip borders, compress tables)
    #[command(disable_help_flag = true, next_display_order = None)]
    Psql {
        /// psql arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Download with compact output (strips progress bars)
    #[command(next_display_order = None)]
    Wget {
        /// URL to download
        url: String,
        /// Output file (-O - for stdout)
        #[arg(short = 'O', long = "output-document", allow_hyphen_values = true)]
        output: Option<String>,
        /// Additional wget arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // ── Files & Search ────────────────────────────────────────────────────────
    /// Ultra-condensed diff (only changed lines)
    #[command(next_display_order = None)]
    Diff {
        /// First file or - for stdin (unified diff)
        file1: PathBuf,
        /// Second file (optional if stdin)
        file2: Option<PathBuf>,
    },

    /// Find files with compact tree output (accepts native find flags like -name, -type)
    #[command(next_display_order = None)]
    Find {
        /// All find arguments (supports both RTK and native find syntax)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Compact grep - strips whitespace, truncates, groups by file
    #[command(next_display_order = None)]
    Grep {
        /// Pattern to search
        pattern: String,
        /// Path to search in
        #[arg(default_value = ".")]
        path: String,
        /// Max line length
        #[arg(short = 'l', long, default_value = "80")]
        max_len: usize,
        /// Max results to show
        #[arg(short, long, default_value = "200")]
        max: usize,
        /// Show only match context (not full line)
        #[arg(short, long)]
        context_only: bool,
        /// Filter by file type (e.g., ts, py, rust)
        #[arg(short = 't', long)]
        file_type: Option<String>,
        /// Show line numbers (always on, accepted for grep/rg compatibility)
        #[arg(short = 'n', long)]
        line_numbers: bool,
        /// Extra ripgrep arguments (e.g., -i, -A 3, -w, --glob)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra_args: Vec<String>,
    },

    /// List directory contents with token-optimized output (proxy to native ls)
    #[command(next_display_order = None)]
    Ls {
        /// Arguments passed to ls (supports all native ls flags like -l, -a, -h, -R)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Read file with intelligent filtering
    #[command(next_display_order = None)]
    Read {
        /// Files to read (supports multiple, like cat)
        #[arg(required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        /// Filter: none (default, full content), minimal, aggressive
        #[arg(short, long, default_value = "none")]
        level: core::filter::FilterLevel,
        /// Max lines
        #[arg(short, long, conflicts_with = "tail_lines")]
        max_lines: Option<usize>,
        /// Keep only last N lines
        #[arg(long, conflicts_with = "max_lines")]
        tail_lines: Option<usize>,
        /// Show line numbers
        #[arg(short = 'n', long)]
        line_numbers: bool,
    },

    /// Directory tree with token-optimized output (proxy to native tree)
    #[command(next_display_order = None)]
    Tree {
        /// Arguments passed to tree (supports all native tree flags like -L, -d, -a)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Word/line/byte count with compact output (strips paths and padding)
    #[command(next_display_order = None)]
    Wc {
        /// Arguments passed to wc (files, flags like -l, -w, -c)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    // ── Frameworks ────────────────────────────────────────────────────────────
    /// Next.js build with compact output
    #[command(next_display_order = None)]
    Next {
        /// Next.js build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Prisma commands with compact output (no ASCII art)
    #[command(next_display_order = None)]
    Prisma {
        #[command(subcommand)]
        command: PrismaCommands,
    },

    // ── Git & GitHub ──────────────────────────────────────────────────────────
    /// GitHub CLI (gh) commands with token-optimized output
    #[command(next_display_order = None)]
    Gh {
        /// Subcommand: pr, issue, run, repo
        subcommand: String,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Git commands with compact output
    #[command(next_display_order = None)]
    Git {
        /// Change to directory before executing (like git -C <path>, can be repeated)
        #[arg(short = 'C', action = clap::ArgAction::Append)]
        directory: Vec<String>,

        /// Git configuration override (like git -c key=value, can be repeated)
        #[arg(short = 'c', action = clap::ArgAction::Append)]
        config_override: Vec<String>,

        /// Set the path to the .git directory
        #[arg(long = "git-dir")]
        git_dir: Option<String>,

        /// Set the path to the working tree
        #[arg(long = "work-tree")]
        work_tree: Option<String>,

        /// Disable pager (like git --no-pager)
        #[arg(long = "no-pager")]
        no_pager: bool,

        /// Skip optional locks (like git --no-optional-locks)
        #[arg(long = "no-optional-locks")]
        no_optional_locks: bool,

        /// Treat repository as bare (like git --bare)
        #[arg(long)]
        bare: bool,

        /// Treat pathspecs literally (like git --literal-pathspecs)
        #[arg(long = "literal-pathspecs")]
        literal_pathspecs: bool,

        #[command(subcommand)]
        command: GitCommands,
    },

    /// Graphite (gt) stacked PR commands with compact output
    #[command(next_display_order = None)]
    Gt {
        #[command(subcommand)]
        command: GtCommands,
    },

    // ── Package Managers ──────────────────────────────────────────────────────
    /// npm run with filtered output (strip boilerplate)
    #[command(next_display_order = None)]
    Npm {
        /// npm run arguments (script name + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// npx with intelligent routing (tsc, eslint, prisma -> specialized filters)
    #[command(next_display_order = None)]
    Npx {
        /// npx arguments (command + options)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Pip package manager with compact output (auto-detects uv)
    #[command(next_display_order = None)]
    Pip {
        /// Pip arguments (e.g., list, outdated, install)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// pnpm commands with ultra-compact output
    #[command(next_display_order = None)]
    Pnpm {
        /// pnpm filter arguments (can be repeated: --filter @app1 --filter @app2)
        #[arg(long, short = 'F')]
        filter: Vec<String>,

        #[command(subcommand)]
        command: PnpmCommands,
    },

    // ── Test ──────────────────────────────────────────────────────────────────
    /// Playwright E2E tests with compact output
    #[command(next_display_order = None)]
    Playwright {
        /// Playwright arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Pytest test runner with compact output
    #[command(next_display_order = None)]
    Pytest {
        /// Pytest arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Rake/Rails test with compact Minitest output (Ruby)
    #[command(next_display_order = None)]
    Rake {
        /// Rake arguments (e.g., test, test TEST=path/to/test.rb)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// RSpec test runner with compact output (Rails/Ruby)
    #[command(next_display_order = None)]
    Rspec {
        /// RSpec arguments (e.g., spec/models, --tag focus)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Run tests and show only failures
    #[command(next_display_order = None)]
    Test {
        /// Test command (e.g. cargo test)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Vitest commands with compact output
    #[command(next_display_order = None)]
    Vitest {
        #[command(subcommand)]
        command: VitestCommands,
    },
}

#[derive(Subcommand)]
enum HookCommands {
    /// Process Gemini CLI BeforeTool hook (reads JSON from stdin)
    #[command(next_display_order = None)]
    Gemini,
    /// Process Copilot preToolUse hook (VS Code + Copilot CLI, reads JSON from stdin)
    #[command(next_display_order = None)]
    Copilot,
}

#[derive(Subcommand)]
enum GitCommands {
    /// Condensed diff output
    #[command(next_display_order = None)]
    Diff {
        /// Git arguments (supports all git diff flags like --stat, --cached, etc)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// One-line commit history
    #[command(next_display_order = None)]
    Log {
        /// Git arguments (supports all git log flags like --oneline, --graph, --all)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact status (supports all git status flags)
    #[command(next_display_order = None)]
    Status {
        /// Git arguments (supports all git status flags like --porcelain, --short, -s)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact show (commit summary + stat + compacted diff)
    #[command(next_display_order = None)]
    Show {
        /// Git arguments (supports all git show flags)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Add files → "ok"
    #[command(next_display_order = None)]
    Add {
        /// Files and flags to add (supports all git add flags like -A, -p, --all, etc)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Commit → "ok \<hash\>"
    #[command(next_display_order = None)]
    Commit {
        /// Git commit arguments (supports -a, -m, --amend, --allow-empty, etc)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Push → "ok \<branch\>"
    #[command(next_display_order = None)]
    Push {
        /// Git push arguments (supports -u, remote, branch, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Pull → "ok \<stats\>"
    #[command(next_display_order = None)]
    Pull {
        /// Git pull arguments (supports --rebase, remote, branch, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact branch listing (current/local/remote)
    #[command(next_display_order = None)]
    Branch {
        /// Git branch arguments (supports -d, -D, -m, etc.)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Fetch → "ok fetched (N new refs)"
    #[command(next_display_order = None)]
    Fetch {
        /// Git fetch arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Stash management (list, show, pop, apply, drop)
    #[command(next_display_order = None)]
    Stash {
        /// Subcommand: list, show, pop, apply, drop, push
        subcommand: Option<String>,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact worktree listing
    #[command(next_display_order = None)]
    Worktree {
        /// Git worktree arguments (add, remove, prune, or empty for list)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported git subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum PnpmCommands {
    /// List installed packages (ultra-dense)
    #[command(next_display_order = None)]
    List {
        /// Depth level (default: 0)
        #[arg(short, long, default_value = "0")]
        depth: usize,
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Show outdated packages (condensed: "pkg: old → new")
    #[command(next_display_order = None)]
    Outdated {
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install packages (filter progress bars)
    #[command(next_display_order = None)]
    Install {
        /// Packages to install
        packages: Vec<String>,
        /// Additional pnpm arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Typecheck (delegates to tsc filter)
    #[command(next_display_order = None)]
    Typecheck {
        /// Additional typecheck arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported pnpm subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum DockerCommands {
    /// List running containers
    #[command(next_display_order = None)]
    Ps,
    /// List images
    #[command(next_display_order = None)]
    Images,
    /// Show container logs (deduplicated)
    #[command(next_display_order = None)]
    Logs { container: String },
    /// Docker Compose commands with compact output
    #[command(next_display_order = None)]
    Compose {
        #[command(subcommand)]
        command: ComposeCommands,
    },
    /// Passthrough: runs any unsupported docker subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum ComposeCommands {
    /// List compose services (compact)
    #[command(next_display_order = None)]
    Ps,
    /// Show compose logs (deduplicated)
    #[command(next_display_order = None)]
    Logs {
        /// Optional service name
        service: Option<String>,
    },
    /// Build compose services (summary)
    #[command(next_display_order = None)]
    Build {
        /// Optional service name
        service: Option<String>,
    },
    /// Passthrough: runs any unsupported compose subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum KubectlCommands {
    /// List pods
    #[command(next_display_order = None)]
    Pods {
        #[arg(short, long)]
        namespace: Option<String>,
        /// All namespaces
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// List services
    #[command(next_display_order = None)]
    Services {
        #[arg(short, long)]
        namespace: Option<String>,
        /// All namespaces
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// Show pod logs (deduplicated)
    #[command(next_display_order = None)]
    Logs {
        pod: String,
        #[arg(short, long)]
        container: Option<String>,
    },
    /// Passthrough: runs any unsupported kubectl subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum VitestCommands {
    /// Run tests with filtered output (90% token reduction)
    #[command(next_display_order = None)]
    Run {
        /// Additional vitest arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PrismaCommands {
    /// Generate Prisma Client (strip ASCII art)
    #[command(next_display_order = None)]
    Generate {
        /// Additional prisma arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage migrations
    #[command(next_display_order = None)]
    Migrate {
        #[command(subcommand)]
        command: PrismaMigrateCommands,
    },
    /// Push schema to database
    #[command(next_display_order = None)]
    DbPush {
        /// Additional prisma arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum PrismaMigrateCommands {
    /// Create and apply migration
    #[command(next_display_order = None)]
    Dev {
        /// Migration name
        #[arg(short, long)]
        name: Option<String>,
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check migration status
    #[command(next_display_order = None)]
    Status {
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Deploy migrations to production
    #[command(next_display_order = None)]
    Deploy {
        /// Additional arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum CargoCommands {
    /// Build with compact output (strip Compiling lines, keep errors)
    #[command(next_display_order = None)]
    Build {
        /// Additional cargo build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Test with failures-only output
    #[command(next_display_order = None)]
    Test {
        /// Additional cargo test arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Clippy with warnings grouped by lint rule
    #[command(next_display_order = None)]
    Clippy {
        /// Additional cargo clippy arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Check with compact output (strip Checking lines, keep errors)
    #[command(next_display_order = None)]
    Check {
        /// Additional cargo check arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Install with compact output (strip dep compilation, keep installed/errors)
    #[command(next_display_order = None)]
    Install {
        /// Additional cargo install arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Nextest with failures-only output
    #[command(next_display_order = None)]
    Nextest {
        /// Additional cargo nextest arguments (e.g., run, list, --lib)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported cargo subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum DotnetCommands {
    /// Build with compact output
    #[command(next_display_order = None)]
    Build {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Test with compact output
    #[command(next_display_order = None)]
    Test {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Restore with compact output
    #[command(next_display_order = None)]
    Restore {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Format with compact output
    #[command(next_display_order = None)]
    Format {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported dotnet subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

#[derive(Subcommand)]
enum GoCommands {
    /// Run tests with compact output (90% token reduction via JSON streaming)
    #[command(next_display_order = None)]
    Test {
        /// Additional go test arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Build with compact output (errors only)
    #[command(next_display_order = None)]
    Build {
        /// Additional go build arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Vet with compact output
    #[command(next_display_order = None)]
    Vet {
        /// Additional go vet arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: runs any unsupported go subcommand directly
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

/// RTK-only subcommands that should never fall back to raw execution.
/// If Clap fails to parse these, show the Clap error directly.
const RTK_META_COMMANDS: &[&str] = &[
    "gain",
    "discover",
    "learn",
    "init",
    "config",
    "proxy",
    "hook-audit",
    "cc-economics",
    "verify",
    "telemetry",
    "trust",
    "untrust",
    "session",
    "rewrite",
];

fn run_fallback(parse_error: clap::Error) -> Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // No args → show Clap's error (user ran just "rtk" with bad syntax)
    if args.is_empty() {
        parse_error.exit();
    }

    // RTK meta-commands should never fall back to raw execution.
    // e.g. `rtk gain --badtypo` should show Clap's error, not try to run `gain` from $PATH.
    if RTK_META_COMMANDS.contains(&args[0].as_str()) {
        parse_error.exit();
    }

    let raw_command = args.join(" ");
    let error_message = core::utils::strip_ansi(&parse_error.to_string());

    // Start timer before execution to capture actual command runtime
    let timer = core::tracking::TimedExecution::start();

    // TOML filter lookup — bypass with RTK_NO_TOML=1
    // Use basename of args[0] so absolute paths (/usr/bin/make) still match "^make\b".
    let lookup_cmd = {
        let base = std::path::Path::new(&args[0])
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| args[0].clone());
        std::iter::once(base.as_str())
            .chain(args[1..].iter().map(|s| s.as_str()))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let toml_match = if std::env::var("RTK_NO_TOML").ok().as_deref() == Some("1") {
        None
    } else {
        core::toml_filter::find_matching_filter(&lookup_cmd)
    };

    if let Some(filter) = toml_match {
        // TOML match: capture stdout for filtering
        let result = if filter.filter_stderr {
            // Merge stderr into stdout so the filter can strip banners emitted by tools like liquibase
            core::utils::resolved_command(&args[0])
                .args(&args[1..])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped()) // captured for merging
                .output()
        } else {
            core::utils::resolved_command(&args[0])
                .args(&args[1..])
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::piped()) // capture
                .stderr(std::process::Stdio::inherit()) // stderr always direct
                .output()
        };

        match result {
            Ok(output) => {
                let exit_code = core::utils::exit_code_from_output(&output, &raw_command);
                let stdout_raw = String::from_utf8_lossy(&output.stdout);
                let stderr_raw = String::from_utf8_lossy(&output.stderr);

                // Merge stderr into the text to filter when filter_stderr is enabled;
                // otherwise emit stderr directly so it is always visible.
                let combined_raw = if filter.filter_stderr {
                    format!("{}{}", stdout_raw, stderr_raw)
                } else {
                    stdout_raw.to_string()
                };
                // Tee raw output BEFORE filtering on failure — lets LLM re-read if needed
                let tee_hint = if !output.status.success() {
                    core::tee::tee_and_hint(&combined_raw, &raw_command, exit_code)
                } else {
                    None
                };

                let filtered = core::toml_filter::apply_filter(filter, &combined_raw);
                println!("{}", filtered);
                if let Some(hint) = tee_hint {
                    println!("{}", hint);
                }

                timer.track(
                    &raw_command,
                    &format!("rtk:toml {}", raw_command),
                    &combined_raw,
                    &filtered,
                );
                core::tracking::record_parse_failure_silent(&raw_command, &error_message, true);

                Ok(exit_code)
            }
            Err(e) => {
                // Command not found — same behaviour as no-TOML path
                core::tracking::record_parse_failure_silent(&raw_command, &error_message, false);
                eprintln!("[rtk: {}]", e);
                Ok(127)
            }
        }
    } else {
        // No TOML match: original passthrough behaviour (Stdio::inherit, streaming)
        let status = core::utils::resolved_command(&args[0])
            .args(&args[1..])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status();

        match status {
            Ok(s) => {
                timer.track_passthrough(&raw_command, &format!("rtk fallback: {}", raw_command));

                core::tracking::record_parse_failure_silent(&raw_command, &error_message, true);

                Ok(core::utils::exit_code_from_status(&s, &raw_command))
            }
            Err(e) => {
                core::tracking::record_parse_failure_silent(&raw_command, &error_message, false);
                // Command not found or other OS error — single message, no duplicate Clap error
                eprintln!("[rtk: {}]", e);
                Ok(127)
            }
        }
    }
}

#[derive(Subcommand)]
enum GtCommands {
    /// Compact stack log output
    #[command(next_display_order = None)]
    Log {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact submit output
    #[command(next_display_order = None)]
    Submit {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact sync output
    #[command(next_display_order = None)]
    Sync {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact restack output
    #[command(next_display_order = None)]
    Restack {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Compact create output
    #[command(next_display_order = None)]
    Create {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Branch info and management
    #[command(next_display_order = None)]
    Branch {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Passthrough: git-passthrough detection or direct gt execution
    #[command(external_subcommand)]
    Other(Vec<OsString>),
}

/// Split a string into shell-like tokens, respecting single and double quotes.
/// e.g. `git log --format="%H %s"` → ["git", "log", "--format=%H %s"]
fn shell_split(input: &str) -> Vec<String> {
    discover::lexer::shell_split(input)
}

/// Merge pnpm global filters args with other ones for standard String-based commands
fn merge_pnpm_args(filters: &[String], args: &[String]) -> Vec<String> {
    filters
        .iter()
        .map(|filter| format!("--filter={}", filter))
        .chain(args.iter().cloned())
        .collect()
}

/// Merge pnpm global filters args with other ones, using OsString for passthrough compatibility
fn merge_pnpm_args_os(filters: &[String], args: &[OsString]) -> Vec<OsString> {
    filters
        .iter()
        .map(|filter| OsString::from(format!("--filter={}", filter)))
        .chain(args.iter().cloned())
        .collect()
}

/// Validate that pnpm filters are only used in the global context, not before subcommands like tsc.
fn validate_pnpm_filters(filters: &[String], command: &PnpmCommands) -> Option<String> {
    // Check if this is a Build or Typecheck command with filters
    match command {
        PnpmCommands::Typecheck { .. } => {
            // FIXME: if filters are present, we should find out which workspaces are selected before running rtk dedicated commands
            if !filters.is_empty() {
                let cmd_name = match command {
                    PnpmCommands::Typecheck { .. } => "tsc",
                    _ => unreachable!(),
                };
                let msg = format!(
                    "[rtk] warning: --filter is not yet supported for pnpm {}, filters preceding the subcommand will be ignored",
                    cmd_name
                );
                return Some(msg);
            }
            None
        }
        _ => None,
    }
}

fn main() {
    let code = match run_cli() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("rtk: {:#}", e);
            1
        }
    };
    std::process::exit(code);
}

fn run_cli() -> Result<i32> {
    // Fire-and-forget telemetry ping (1/day, non-blocking)
    core::telemetry::maybe_ping();

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                e.exit();
            }
            return run_fallback(e);
        }
    };

    // Warn if installed hook is outdated/missing (1/day, non-blocking).
    // Skip for Gain — it shows its own inline hook warning.
    if !matches!(cli.command, Commands::Gain { .. }) {
        hooks::hook_check::maybe_warn();
    }

    // Runtime integrity check for operational commands.
    // Meta commands (init, gain, verify, config, etc.) skip the check
    // because they don't go through the hook pipeline.
    if is_operational_command(&cli.command) {
        hooks::integrity::runtime_check()?;
    }

    let code = match cli.command {
        Commands::Ls { args } => ls::run(&args, cli.verbose)?,

        Commands::Tree { args } => tree::run(&args, cli.verbose)?,

        // ISSUE #989: support multiple files (cat file1 file2 → rtk read file1 file2)
        Commands::Read {
            files,
            level,
            max_lines,
            tail_lines,
            line_numbers,
        } => {
            let mut had_error = false;
            let mut stdin_seen = false;
            for file in &files {
                let result = if file == Path::new("-") {
                    if stdin_seen {
                        eprintln!("rtk: warning: stdin specified more than once");
                        continue;
                    }
                    stdin_seen = true;
                    read::run_stdin(level, max_lines, tail_lines, line_numbers, cli.verbose)
                } else {
                    read::run(
                        file,
                        level,
                        max_lines,
                        tail_lines,
                        line_numbers,
                        cli.verbose,
                    )
                };
                if let Err(e) = result {
                    eprintln!("cat: {}: {}", file.display(), e.root_cause());
                    had_error = true;
                }
            }
            if had_error {
                1
            } else {
                0
            }
        }

        Commands::Smart {
            file,
            model,
            force_download,
        } => {
            local_llm::run(&file, &model, force_download, cli.verbose)?;
            0
        }

        Commands::Git {
            directory,
            config_override,
            git_dir,
            work_tree,
            no_pager,
            no_optional_locks,
            bare,
            literal_pathspecs,
            command,
        } => {
            // Build global git args (inserted between "git" and subcommand)
            let mut global_args: Vec<String> = Vec::new();
            for dir in &directory {
                global_args.push("-C".to_string());
                global_args.push(dir.clone());
            }
            for cfg in &config_override {
                global_args.push("-c".to_string());
                global_args.push(cfg.clone());
            }
            if let Some(ref dir) = git_dir {
                global_args.push("--git-dir".to_string());
                global_args.push(dir.clone());
            }
            if let Some(ref tree) = work_tree {
                global_args.push("--work-tree".to_string());
                global_args.push(tree.clone());
            }
            if no_pager {
                global_args.push("--no-pager".to_string());
            }
            if no_optional_locks {
                global_args.push("--no-optional-locks".to_string());
            }
            if bare {
                global_args.push("--bare".to_string());
            }
            if literal_pathspecs {
                global_args.push("--literal-pathspecs".to_string());
            }

            match command {
                GitCommands::Diff { args } => git::run(
                    git::GitCommand::Diff,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Log { args } => {
                    git::run(git::GitCommand::Log, &args, None, cli.verbose, &global_args)?
                }
                GitCommands::Status { args } => git::run(
                    git::GitCommand::Status,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Show { args } => git::run(
                    git::GitCommand::Show,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Add { args } => {
                    git::run(git::GitCommand::Add, &args, None, cli.verbose, &global_args)?
                }
                GitCommands::Commit { args } => git::run(
                    git::GitCommand::Commit,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Push { args } => git::run(
                    git::GitCommand::Push,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Pull { args } => git::run(
                    git::GitCommand::Pull,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Branch { args } => git::run(
                    git::GitCommand::Branch,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Fetch { args } => git::run(
                    git::GitCommand::Fetch,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Stash { subcommand, args } => git::run(
                    git::GitCommand::Stash { subcommand },
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Worktree { args } => git::run(
                    git::GitCommand::Worktree,
                    &args,
                    None,
                    cli.verbose,
                    &global_args,
                )?,
                GitCommands::Other(args) => git::run_passthrough(&args, &global_args, cli.verbose)?,
            }
        }

        Commands::Gh { subcommand, args } => {
            gh_cmd::run(&subcommand, &args, cli.verbose, cli.ultra_compact)?
        }

        Commands::Aws { subcommand, args } => aws_cmd::run(&subcommand, &args, cli.verbose)?,

        Commands::Psql { args } => psql_cmd::run(&args, cli.verbose)?,

        Commands::Pnpm { filter, command } => {
            // Warns user if filters are used with unsupported subcommands like typecheck
            if let Some(warning) = validate_pnpm_filters(&filter, &command) {
                eprintln!("{}", warning);
            }

            match command {
                PnpmCommands::List { depth, args } => pnpm_cmd::run(
                    pnpm_cmd::PnpmCommand::List { depth },
                    &merge_pnpm_args(&filter, &args),
                    cli.verbose,
                )?,
                PnpmCommands::Outdated { args } => pnpm_cmd::run(
                    pnpm_cmd::PnpmCommand::Outdated,
                    &merge_pnpm_args(&filter, &args),
                    cli.verbose,
                )?,
                PnpmCommands::Install { packages, args } => pnpm_cmd::run(
                    pnpm_cmd::PnpmCommand::Install { packages },
                    &merge_pnpm_args(&filter, &args),
                    cli.verbose,
                )?,
                PnpmCommands::Typecheck { args } => tsc_cmd::run(&args, cli.verbose)?,
                PnpmCommands::Other(args) => {
                    pnpm_cmd::run_passthrough(&merge_pnpm_args_os(&filter, &args), cli.verbose)?
                }
            }
        }

        Commands::Err { command } => {
            let cmd = command.join(" ");
            runner::run_err(&cmd, cli.verbose)?
        }

        Commands::Test { command } => {
            let cmd = command.join(" ");
            runner::run_test(&cmd, cli.verbose)?
        }

        Commands::Json {
            file,
            depth,
            keys_only,
        } => {
            if file == Path::new("-") {
                json_cmd::run_stdin(depth, keys_only, cli.verbose)?;
            } else {
                json_cmd::run(&file, depth, keys_only, cli.verbose)?;
            }
            0
        }

        Commands::Deps { path } => {
            deps::run(&path, cli.verbose)?;
            0
        }

        Commands::Env { filter, show_all } => {
            env_cmd::run(filter.as_deref(), show_all, cli.verbose)?;
            0
        }

        Commands::Find { args } => {
            find_cmd::run_from_args(&args, cli.verbose)?;
            0
        }

        Commands::Diff { file1, file2 } => {
            if let Some(f2) = file2 {
                diff_cmd::run(&file1, &f2, cli.verbose)?;
            } else {
                diff_cmd::run_stdin(cli.verbose)?;
            }
            0
        }

        Commands::Log { file } => {
            if let Some(f) = file {
                log_cmd::run_file(&f, cli.verbose)?;
            } else {
                log_cmd::run_stdin(cli.verbose)?;
            }
            0
        }

        Commands::Dotnet { command } => match command {
            DotnetCommands::Build { args } => dotnet_cmd::run_build(&args, cli.verbose)?,
            DotnetCommands::Test { args } => dotnet_cmd::run_test(&args, cli.verbose)?,
            DotnetCommands::Restore { args } => dotnet_cmd::run_restore(&args, cli.verbose)?,
            DotnetCommands::Format { args } => dotnet_cmd::run_format(&args, cli.verbose)?,
            DotnetCommands::Other(args) => dotnet_cmd::run_passthrough(&args, cli.verbose)?,
        },

        Commands::Docker { command } => match command {
            DockerCommands::Ps => {
                container::run(container::ContainerCmd::DockerPs, &[], cli.verbose)?
            }
            DockerCommands::Images => {
                container::run(container::ContainerCmd::DockerImages, &[], cli.verbose)?
            }
            DockerCommands::Logs { container: c } => {
                container::run(container::ContainerCmd::DockerLogs, &[c], cli.verbose)?
            }
            DockerCommands::Compose { command: compose } => match compose {
                ComposeCommands::Ps => container::run_compose_ps(cli.verbose)?,
                ComposeCommands::Logs { service } => {
                    container::run_compose_logs(service.as_deref(), cli.verbose)?
                }
                ComposeCommands::Build { service } => {
                    container::run_compose_build(service.as_deref(), cli.verbose)?
                }
                ComposeCommands::Other(args) => {
                    container::run_compose_passthrough(&args, cli.verbose)?
                }
            },
            DockerCommands::Other(args) => container::run_docker_passthrough(&args, cli.verbose)?,
        },

        Commands::Kubectl { command } => match command {
            KubectlCommands::Pods { namespace, all } => {
                let mut args: Vec<String> = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(n) = namespace {
                    args.push("-n".to_string());
                    args.push(n);
                }
                container::run(container::ContainerCmd::KubectlPods, &args, cli.verbose)?
            }
            KubectlCommands::Services { namespace, all } => {
                let mut args: Vec<String> = Vec::new();
                if all {
                    args.push("-A".to_string());
                } else if let Some(n) = namespace {
                    args.push("-n".to_string());
                    args.push(n);
                }
                container::run(container::ContainerCmd::KubectlServices, &args, cli.verbose)?
            }
            KubectlCommands::Logs { pod, container: c } => {
                let mut args = vec![pod];
                if let Some(cont) = c {
                    args.push("-c".to_string());
                    args.push(cont);
                }
                container::run(container::ContainerCmd::KubectlLogs, &args, cli.verbose)?
            }
            KubectlCommands::Other(args) => container::run_kubectl_passthrough(&args, cli.verbose)?,
        },

        Commands::Summary { command } => {
            let cmd = command.join(" ");
            summary::run(&cmd, cli.verbose)?
        }

        Commands::Grep {
            pattern,
            path,
            max_len,
            max,
            context_only,
            file_type,
            line_numbers: _, // no-op: line numbers always enabled in grep_cmd::run
            extra_args,
        } => grep_cmd::run(
            &pattern,
            &path,
            max_len,
            max,
            context_only,
            file_type.as_deref(),
            &extra_args,
            cli.verbose,
        )?,

        Commands::Init {
            global,
            opencode,
            gemini,
            agent,
            show,
            claude_md,
            hook_only,
            auto_patch,
            no_patch,
            uninstall,
            codex,
            copilot,
        } => {
            if show {
                hooks::init::show_config(codex)?;
            } else if uninstall {
                let cursor = agent == Some(AgentTarget::Cursor);
                hooks::init::uninstall(global, gemini, codex, cursor, cli.verbose)?;
            } else if gemini {
                let patch_mode = if auto_patch {
                    hooks::init::PatchMode::Auto
                } else if no_patch {
                    hooks::init::PatchMode::Skip
                } else {
                    hooks::init::PatchMode::Ask
                };
                hooks::init::run_gemini(global, hook_only, patch_mode, cli.verbose)?;
            } else if copilot {
                hooks::init::run_copilot(cli.verbose)?;
            } else if agent == Some(AgentTarget::Kilocode) {
                if global {
                    anyhow::bail!("Kilo Code is project-scoped. Use: rtk init --agent kilocode");
                }
                hooks::init::run_kilocode_mode(cli.verbose)?;
            } else if agent == Some(AgentTarget::Antigravity) {
                if global {
                    anyhow::bail!(
                        "Antigravity is project-scoped. Use: rtk init --agent antigravity"
                    );
                }
                hooks::init::run_antigravity_mode(cli.verbose)?;
            } else {
                let install_opencode = opencode;
                let install_claude = !opencode;
                let install_cursor = agent == Some(AgentTarget::Cursor);
                let install_windsurf = agent == Some(AgentTarget::Windsurf);
                let install_cline = agent == Some(AgentTarget::Cline);

                let patch_mode = if auto_patch {
                    hooks::init::PatchMode::Auto
                } else if no_patch {
                    hooks::init::PatchMode::Skip
                } else {
                    hooks::init::PatchMode::Ask
                };
                hooks::init::run(
                    global,
                    install_claude,
                    install_opencode,
                    install_cursor,
                    install_windsurf,
                    install_cline,
                    claude_md,
                    hook_only,
                    codex,
                    patch_mode,
                    cli.verbose,
                )?;
            }
            0
        }

        Commands::Wget { url, output, args } => {
            if output.as_deref() == Some("-") {
                wget_cmd::run_stdout(&url, &args, cli.verbose)?
            } else {
                // Pass -O <file> through to wget via args
                let mut all_args = Vec::new();
                if let Some(out_file) = &output {
                    all_args.push("-O".to_string());
                    all_args.push(out_file.clone());
                }
                all_args.extend(args);
                wget_cmd::run(&url, &all_args, cli.verbose)?
            }
        }

        Commands::Wc { args } => wc_cmd::run(&args, cli.verbose)?,

        Commands::Gain {
            project,
            graph,
            history,
            quota,
            tier,
            daily,
            weekly,
            monthly,
            all,
            format,
            failures,
        } => {
            analytics::gain::run(
                project,
                graph,
                history,
                quota,
                &tier,
                daily,
                weekly,
                monthly,
                all,
                &format,
                failures,
                cli.verbose,
            )?;
            0
        }

        Commands::CcEconomics {
            daily,
            weekly,
            monthly,
            all,
            format,
        } => {
            analytics::cc_economics::run(daily, weekly, monthly, all, &format, cli.verbose)?;
            0
        }

        Commands::Config { create } => {
            if create {
                let path = core::config::Config::create_default()?;
                println!("Created: {}", path.display());
            } else {
                core::config::show_config()?;
            }
            0
        }

        Commands::Vitest { command } => match command {
            VitestCommands::Run { args } => {
                vitest_cmd::run(vitest_cmd::VitestCommand::Run, &args, cli.verbose)?
            }
        },

        Commands::Prisma { command } => match command {
            PrismaCommands::Generate { args } => {
                prisma_cmd::run(prisma_cmd::PrismaCommand::Generate, &args, cli.verbose)?
            }
            PrismaCommands::Migrate { command } => match command {
                PrismaMigrateCommands::Dev { name, args } => prisma_cmd::run(
                    prisma_cmd::PrismaCommand::Migrate {
                        subcommand: prisma_cmd::MigrateSubcommand::Dev { name },
                    },
                    &args,
                    cli.verbose,
                )?,
                PrismaMigrateCommands::Status { args } => prisma_cmd::run(
                    prisma_cmd::PrismaCommand::Migrate {
                        subcommand: prisma_cmd::MigrateSubcommand::Status,
                    },
                    &args,
                    cli.verbose,
                )?,
                PrismaMigrateCommands::Deploy { args } => prisma_cmd::run(
                    prisma_cmd::PrismaCommand::Migrate {
                        subcommand: prisma_cmd::MigrateSubcommand::Deploy,
                    },
                    &args,
                    cli.verbose,
                )?,
            },
            PrismaCommands::DbPush { args } => {
                prisma_cmd::run(prisma_cmd::PrismaCommand::DbPush, &args, cli.verbose)?
            }
        },

        Commands::Tsc { args } => tsc_cmd::run(&args, cli.verbose)?,

        Commands::Next { args } => next_cmd::run(&args, cli.verbose)?,

        Commands::Lint { args } => lint_cmd::run(&args, cli.verbose)?,

        Commands::Prettier { args } => prettier_cmd::run(&args, cli.verbose)?,

        Commands::Format { args } => format_cmd::run(&args, cli.verbose)?,

        Commands::Playwright { args } => playwright_cmd::run(&args, cli.verbose)?,

        Commands::Cargo { command } => match command {
            CargoCommands::Build { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Build, &args, cli.verbose)?
            }
            CargoCommands::Test { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Test, &args, cli.verbose)?
            }
            CargoCommands::Clippy { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Clippy, &args, cli.verbose)?
            }
            CargoCommands::Check { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Check, &args, cli.verbose)?
            }
            CargoCommands::Install { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Install, &args, cli.verbose)?
            }
            CargoCommands::Nextest { args } => {
                cargo_cmd::run(cargo_cmd::CargoCommand::Nextest, &args, cli.verbose)?
            }
            CargoCommands::Other(args) => cargo_cmd::run_passthrough(&args, cli.verbose)?,
        },

        Commands::Npm { args } => npm_cmd::run(&args, cli.verbose, cli.skip_env)?,

        Commands::Curl { args } => curl_cmd::run(&args, cli.verbose)?,

        Commands::Discover {
            project,
            limit,
            all,
            since,
            format,
        } => {
            discover::run(project.as_deref(), all, since, limit, &format, cli.verbose)?;
            0
        }

        Commands::Session {} => {
            analytics::session_cmd::run(cli.verbose)?;
            0
        }

        Commands::Telemetry { command } => {
            core::telemetry_cmd::run(&command)?;
            0
        }

        Commands::Learn {
            project,
            all,
            since,
            format,
            write_rules,
            min_confidence,
            min_occurrences,
        } => {
            learn::run(
                project,
                all,
                since,
                format,
                write_rules,
                min_confidence,
                min_occurrences,
            )?;
            0
        }

        Commands::Npx { args } => {
            if args.is_empty() {
                anyhow::bail!("npx requires a command argument");
            }

            // Intelligent routing: delegate to specialized filters
            match args[0].as_str() {
                "tsc" | "typescript" => tsc_cmd::run(&args[1..], cli.verbose)?,
                "eslint" => lint_cmd::run(&args[1..], cli.verbose)?,
                "prisma" => {
                    // Route to prisma_cmd based on subcommand
                    if args.len() > 1 {
                        let prisma_args: Vec<String> = args[2..].to_vec();
                        match args[1].as_str() {
                            "generate" => prisma_cmd::run(
                                prisma_cmd::PrismaCommand::Generate,
                                &prisma_args,
                                cli.verbose,
                            )?,
                            "db" if args.len() > 2 && args[2] == "push" => prisma_cmd::run(
                                prisma_cmd::PrismaCommand::DbPush,
                                &args[3..],
                                cli.verbose,
                            )?,
                            _ => {
                                // Passthrough other prisma subcommands
                                let timer = core::tracking::TimedExecution::start();
                                let mut cmd = core::utils::resolved_command("npx");
                                for arg in &args {
                                    cmd.arg(arg);
                                }
                                let status = cmd.status().context("Failed to run npx prisma")?;
                                let args_str = args.join(" ");
                                timer.track_passthrough(
                                    &format!("npx {}", args_str),
                                    &format!("rtk npx {} (passthrough)", args_str),
                                );
                                core::utils::exit_code_from_status(&status, "npx prisma")
                            }
                        }
                    } else {
                        let timer = core::tracking::TimedExecution::start();
                        let status = core::utils::resolved_command("npx")
                            .arg("prisma")
                            .status()
                            .context("Failed to run npx prisma")?;
                        timer.track_passthrough("npx prisma", "rtk npx prisma (passthrough)");
                        core::utils::exit_code_from_status(&status, "npx prisma")
                    }
                }
                "next" => next_cmd::run(&args[1..], cli.verbose)?,
                "prettier" => prettier_cmd::run(&args[1..], cli.verbose)?,
                "playwright" => playwright_cmd::run(&args[1..], cli.verbose)?,
                _ => {
                    // Generic passthrough with npm boilerplate filter
                    npm_cmd::run(&args, cli.verbose, cli.skip_env)?
                }
            }
        }

        Commands::Ruff { args } => ruff_cmd::run(&args, cli.verbose)?,

        Commands::Pytest { args } => pytest_cmd::run(&args, cli.verbose)?,

        Commands::Mypy { args } => mypy_cmd::run(&args, cli.verbose)?,

        Commands::Rake { args } => rake_cmd::run(&args, cli.verbose)?,

        Commands::Rubocop { args } => rubocop_cmd::run(&args, cli.verbose)?,

        Commands::Rspec { args } => rspec_cmd::run(&args, cli.verbose)?,

        Commands::Pip { args } => pip_cmd::run(&args, cli.verbose)?,

        Commands::Go { command } => match command {
            GoCommands::Test { args } => go_cmd::run_test(&args, cli.verbose)?,
            GoCommands::Build { args } => go_cmd::run_build(&args, cli.verbose)?,
            GoCommands::Vet { args } => go_cmd::run_vet(&args, cli.verbose)?,
            GoCommands::Other(args) => go_cmd::run_other(&args, cli.verbose)?,
        },

        Commands::Gt { command } => match command {
            GtCommands::Log { args } => gt_cmd::run_log(&args, cli.verbose)?,
            GtCommands::Submit { args } => gt_cmd::run_submit(&args, cli.verbose)?,
            GtCommands::Sync { args } => gt_cmd::run_sync(&args, cli.verbose)?,
            GtCommands::Restack { args } => gt_cmd::run_restack(&args, cli.verbose)?,
            GtCommands::Create { args } => gt_cmd::run_create(&args, cli.verbose)?,
            GtCommands::Branch { args } => gt_cmd::run_branch(&args, cli.verbose)?,
            GtCommands::Other(args) => gt_cmd::run_other(&args, cli.verbose)?,
        },

        Commands::GolangciLint { args } => golangci_cmd::run(&args, cli.verbose)?,

        Commands::HookAudit { since } => {
            hooks::hook_audit_cmd::run(since, cli.verbose)?;
            0
        }

        Commands::Hook { command } => {
            match command {
                HookCommands::Gemini => hooks::hook_cmd::run_gemini()?,
                HookCommands::Copilot => hooks::hook_cmd::run_copilot()?,
            }
            0
        }

        Commands::Rewrite { args } => {
            let cmd = args.join(" ");
            hooks::rewrite_cmd::run(&cmd)?;
            0
        }

        Commands::Proxy { args } => {
            use std::io::{Read, Write};
            use std::process::Stdio;
            use std::sync::atomic::{AtomicU32, Ordering};
            use std::thread;

            if args.is_empty() {
                anyhow::bail!(
                    "proxy requires a command to execute\nUsage: rtk proxy <command> [args...]"
                );
            }

            let timer = core::tracking::TimedExecution::start();

            // If a single quoted arg contains spaces, split it respecting quotes (#388).
            // e.g. rtk proxy 'head -50 file.php' → cmd=head, args=["-50", "file.php"]
            // e.g. rtk proxy 'git log --format="%H %s"' → cmd=git, args=["log", "--format=%H %s"]
            let (cmd_name, cmd_args): (String, Vec<String>) = if args.len() == 1 {
                let full = args[0].to_string_lossy();
                let parts = shell_split(&full);
                if parts.len() > 1 {
                    (parts[0].clone(), parts[1..].to_vec())
                } else {
                    (full.into_owned(), vec![])
                }
            } else {
                (
                    args[0].to_string_lossy().into_owned(),
                    args[1..]
                        .iter()
                        .map(|s| s.to_string_lossy().into_owned())
                        .collect(),
                )
            };

            if cli.verbose > 0 {
                eprintln!("Proxy mode: {} {}", cmd_name, cmd_args.join(" "));
            }

            // ISSUE #897: Kill proxy child on SIGINT/SIGTERM to prevent orphan
            // processes. Drop-based ChildGuard doesn't run on signals with
            // panic=abort, so we register a signal handler that kills the child
            // PID stored in this atomic.
            static PROXY_CHILD_PID: AtomicU32 = AtomicU32::new(0);

            #[cfg(unix)]
            {
                unsafe extern "C" fn handle_signal(sig: libc::c_int) {
                    let pid = PROXY_CHILD_PID.load(Ordering::SeqCst);
                    if pid != 0 {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                        libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0);
                    }
                    // Re-raise with default handler so parent sees correct exit status
                    libc::signal(sig, libc::SIG_DFL);
                    libc::raise(sig);
                }
                unsafe {
                    libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
                    libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
                }
            }

            struct ChildGuard(Option<std::process::Child>);
            impl Drop for ChildGuard {
                fn drop(&mut self) {
                    if let Some(mut child) = self.0.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                    PROXY_CHILD_PID.store(0, Ordering::SeqCst);
                }
            }

            let mut child = ChildGuard(Some(
                core::utils::resolved_command(cmd_name.as_ref())
                    .args(&cmd_args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .context(format!("Failed to execute command: {}", cmd_name))?,
            ));

            // Store child PID for signal handler before anything can fail
            if let Some(ref inner) = child.0 {
                PROXY_CHILD_PID.store(inner.id(), Ordering::SeqCst);
            }

            let inner = child.0.as_mut().context("Child process missing")?;
            let stdout_pipe = inner
                .stdout
                .take()
                .context("Failed to capture child stdout")?;
            let stderr_pipe = inner
                .stderr
                .take()
                .context("Failed to capture child stderr")?;

            const CAP: usize = 1_048_576;

            let stdout_handle = thread::spawn(move || -> std::io::Result<Vec<u8>> {
                let mut reader = stdout_pipe;
                let mut captured = Vec::new();
                let mut buf = [0u8; 8192];

                loop {
                    let count = reader.read(&mut buf)?;
                    if count == 0 {
                        break;
                    }
                    if captured.len() < CAP {
                        let take = count.min(CAP - captured.len());
                        captured.extend_from_slice(&buf[..take]);
                    }
                    let mut out = std::io::stdout().lock();
                    out.write_all(&buf[..count])?;
                    out.flush()?;
                }

                Ok(captured)
            });

            let stderr_handle = thread::spawn(move || -> std::io::Result<Vec<u8>> {
                let mut reader = stderr_pipe;
                let mut captured = Vec::new();
                let mut buf = [0u8; 8192];

                loop {
                    let count = reader.read(&mut buf)?;
                    if count == 0 {
                        break;
                    }
                    if captured.len() < CAP {
                        let take = count.min(CAP - captured.len());
                        captured.extend_from_slice(&buf[..take]);
                    }
                    let mut err = std::io::stderr().lock();
                    err.write_all(&buf[..count])?;
                    err.flush()?;
                }

                Ok(captured)
            });

            let status = child
                .0
                .take()
                .context("Child process missing")?
                .wait()
                .context(format!("Failed waiting for command: {}", cmd_name))?;

            let stdout_bytes = stdout_handle
                .join()
                .map_err(|_| anyhow::anyhow!("stdout streaming thread panicked"))??;
            let stderr_bytes = stderr_handle
                .join()
                .map_err(|_| anyhow::anyhow!("stderr streaming thread panicked"))??;

            let stdout = String::from_utf8_lossy(&stdout_bytes);
            let stderr = String::from_utf8_lossy(&stderr_bytes);
            let full_output = format!("{}{}", stdout, stderr);

            // Track usage (input = output since no filtering)
            timer.track(
                &format!("{} {}", cmd_name, cmd_args.join(" ")),
                &format!("rtk proxy {} {}", cmd_name, cmd_args.join(" ")),
                &full_output,
                &full_output,
            );

            core::utils::exit_code_from_status(&status, &cmd_name)
        }

        Commands::Trust { list } => {
            hooks::trust::run_trust(list)?;
            0
        }

        Commands::Untrust => {
            hooks::trust::run_untrust()?;
            0
        }

        Commands::Verify {
            filter,
            require_all,
        } => {
            if filter.is_some() {
                // Filter-specific mode: run only that filter's tests
                hooks::verify_cmd::run(filter, require_all)?;
            } else {
                // Default or --require-all: always run integrity check first
                hooks::integrity::run_verify(cli.verbose)?;
                hooks::verify_cmd::run(None, require_all)?;
            }
            0
        }
    };

    Ok(code)
}

/// Returns true for commands that are invoked via the hook pipeline
/// (i.e., commands that process rewritten shell commands).
/// Meta commands (init, gain, verify, etc.) are excluded because
/// they are run directly by the user, not through the hook.
/// Returns true for commands that go through the hook pipeline
/// and therefore require integrity verification.
///
/// SECURITY: whitelist pattern — new commands are NOT integrity-checked
/// until explicitly added here. A forgotten command fails open (no check)
/// rather than creating false confidence about what's protected.
fn is_operational_command(cmd: &Commands) -> bool {
    matches!(
        cmd,
        Commands::Ls { .. }
            | Commands::Tree { .. }
            | Commands::Read { .. }
            | Commands::Smart { .. }
            | Commands::Git { .. }
            | Commands::Gh { .. }
            | Commands::Pnpm { .. }
            | Commands::Err { .. }
            | Commands::Test { .. }
            | Commands::Json { .. }
            | Commands::Deps { .. }
            | Commands::Env { .. }
            | Commands::Find { .. }
            | Commands::Diff { .. }
            | Commands::Log { .. }
            | Commands::Dotnet { .. }
            | Commands::Docker { .. }
            | Commands::Kubectl { .. }
            | Commands::Summary { .. }
            | Commands::Grep { .. }
            | Commands::Wget { .. }
            | Commands::Vitest { .. }
            | Commands::Prisma { .. }
            | Commands::Tsc { .. }
            | Commands::Next { .. }
            | Commands::Lint { .. }
            | Commands::Prettier { .. }
            | Commands::Playwright { .. }
            | Commands::Cargo { .. }
            | Commands::Npm { .. }
            | Commands::Npx { .. }
            | Commands::Curl { .. }
            | Commands::Ruff { .. }
            | Commands::Pytest { .. }
            | Commands::Rake { .. }
            | Commands::Rubocop { .. }
            | Commands::Rspec { .. }
            | Commands::Pip { .. }
            | Commands::Go { .. }
            | Commands::GolangciLint { .. }
            | Commands::Gt { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    #[test]
    fn test_git_commit_single_message() {
        let cli = Cli::try_parse_from(["rtk", "git", "commit", "-m", "fix: typo"]).unwrap();
        match cli.command {
            Commands::Git {
                command: GitCommands::Commit { args },
                ..
            } => {
                assert_eq!(args, vec!["-m", "fix: typo"]);
            }
            _ => panic!("Expected Git Commit command"),
        }
    }

    #[test]
    fn test_git_commit_multiple_messages() {
        let cli = Cli::try_parse_from([
            "rtk",
            "git",
            "commit",
            "-m",
            "feat: add support",
            "-m",
            "Body paragraph here.",
        ])
        .unwrap();
        match cli.command {
            Commands::Git {
                command: GitCommands::Commit { args },
                ..
            } => {
                assert_eq!(
                    args,
                    vec!["-m", "feat: add support", "-m", "Body paragraph here."]
                );
            }
            _ => panic!("Expected Git Commit command"),
        }
    }

    // #327: git commit -am "msg" was rejected by Clap
    #[test]
    fn test_git_commit_am_flag() {
        let cli = Cli::try_parse_from(["rtk", "git", "commit", "-am", "quick fix"]).unwrap();
        match cli.command {
            Commands::Git {
                command: GitCommands::Commit { args },
                ..
            } => {
                assert_eq!(args, vec!["-am", "quick fix"]);
            }
            _ => panic!("Expected Git Commit command"),
        }
    }

    #[test]
    fn test_git_commit_amend() {
        let cli =
            Cli::try_parse_from(["rtk", "git", "commit", "--amend", "-m", "new msg"]).unwrap();
        match cli.command {
            Commands::Git {
                command: GitCommands::Commit { args },
                ..
            } => {
                assert_eq!(args, vec!["--amend", "-m", "new msg"]);
            }
            _ => panic!("Expected Git Commit command"),
        }
    }

    #[test]
    fn test_git_global_options_parsing() {
        let cli =
            Cli::try_parse_from(["rtk", "git", "--no-pager", "--no-optional-locks", "status"])
                .unwrap();
        match cli.command {
            Commands::Git {
                no_pager,
                no_optional_locks,
                bare,
                literal_pathspecs,
                ..
            } => {
                assert!(no_pager);
                assert!(no_optional_locks);
                assert!(!bare);
                assert!(!literal_pathspecs);
            }
            _ => panic!("Expected Git command"),
        }
    }

    #[test]
    fn test_git_commit_long_flag_multiple() {
        let cli = Cli::try_parse_from([
            "rtk",
            "git",
            "commit",
            "--message",
            "title",
            "--message",
            "body",
            "--message",
            "footer",
        ])
        .unwrap();
        match cli.command {
            Commands::Git {
                command: GitCommands::Commit { args },
                ..
            } => {
                assert_eq!(
                    args,
                    vec![
                        "--message",
                        "title",
                        "--message",
                        "body",
                        "--message",
                        "footer"
                    ]
                );
            }
            _ => panic!("Expected Git Commit command"),
        }
    }

    #[test]
    fn test_try_parse_valid_git_status() {
        let result = Cli::try_parse_from(["rtk", "git", "status"]);
        assert!(result.is_ok(), "git status should parse successfully");
    }

    #[test]
    fn test_try_parse_help_is_display_help() {
        match Cli::try_parse_from(["rtk", "--help"]) {
            Err(e) => assert_eq!(e.kind(), ErrorKind::DisplayHelp),
            Ok(_) => panic!("Expected DisplayHelp error"),
        }
    }

    #[test]
    fn test_try_parse_version_is_display_version() {
        match Cli::try_parse_from(["rtk", "--version"]) {
            Err(e) => assert_eq!(e.kind(), ErrorKind::DisplayVersion),
            Ok(_) => panic!("Expected DisplayVersion error"),
        }
    }

    #[test]
    fn test_try_parse_unknown_subcommand_is_error() {
        match Cli::try_parse_from(["rtk", "nonexistent-command"]) {
            Err(e) => assert!(!matches!(
                e.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            )),
            Ok(_) => panic!("Expected parse error for unknown subcommand"),
        }
    }

    #[test]
    fn test_try_parse_git_with_dash_c_succeeds() {
        let result = Cli::try_parse_from(["rtk", "git", "-C", "/path", "status"]);
        assert!(
            result.is_ok(),
            "git -C /path status should parse successfully"
        );
        if let Ok(cli) = result {
            match cli.command {
                Commands::Git { directory, .. } => {
                    assert_eq!(directory, vec!["/path"]);
                }
                _ => panic!("Expected Git command"),
            }
        }
    }

    #[test]
    fn test_gain_failures_flag_parses() {
        let result = Cli::try_parse_from(["rtk", "gain", "--failures"]);
        assert!(result.is_ok());
        if let Ok(cli) = result {
            match cli.command {
                Commands::Gain { failures, .. } => assert!(failures),
                _ => panic!("Expected Gain command"),
            }
        }
    }

    #[test]
    fn test_gain_failures_short_flag_parses() {
        let result = Cli::try_parse_from(["rtk", "gain", "-F"]);
        assert!(result.is_ok());
        if let Ok(cli) = result {
            match cli.command {
                Commands::Gain { failures, .. } => assert!(failures),
                _ => panic!("Expected Gain command"),
            }
        }
    }

    #[test]
    fn test_meta_commands_reject_bad_flags() {
        // RTK meta-commands should produce parse errors (not fall through to raw execution).
        // Skip "proxy" because it uses trailing_var_arg (accepts any args by design).
        for cmd in RTK_META_COMMANDS {
            if matches!(*cmd, "proxy" | "rewrite" | "session") {
                continue; // these use trailing_var_arg (accept any args by design)
            }
            let result = Cli::try_parse_from(["rtk", cmd, "--nonexistent-flag-xyz"]);
            assert!(
                result.is_err(),
                "Meta-command '{}' with bad flag should fail to parse",
                cmd
            );
        }
    }

    #[test]
    fn test_meta_command_list_is_complete() {
        // Verify all meta-commands are in the guard list by checking they parse with valid syntax
        let meta_cmds_that_parse = [
            vec!["rtk", "gain"],
            vec!["rtk", "discover"],
            vec!["rtk", "learn"],
            vec!["rtk", "init"],
            vec!["rtk", "config"],
            vec!["rtk", "proxy", "echo", "hi"],
            vec!["rtk", "hook-audit"],
            vec!["rtk", "cc-economics"],
        ];
        for args in &meta_cmds_that_parse {
            let result = Cli::try_parse_from(args.iter());
            assert!(
                result.is_ok(),
                "Meta-command {:?} should parse successfully",
                args
            );
        }
    }

    #[test]
    fn test_shell_split_simple() {
        assert_eq!(
            shell_split("head -50 file.php"),
            vec!["head", "-50", "file.php"]
        );
    }

    #[test]
    fn test_shell_split_double_quotes() {
        assert_eq!(
            shell_split(r#"git log --format="%H %s""#),
            vec!["git", "log", "--format=%H %s"]
        );
    }

    #[test]
    fn test_shell_split_single_quotes() {
        assert_eq!(
            shell_split("grep -r 'hello world' ."),
            vec!["grep", "-r", "hello world", "."]
        );
    }

    #[test]
    fn test_shell_split_single_word() {
        assert_eq!(shell_split("ls"), vec!["ls"]);
    }

    #[test]
    fn test_shell_split_empty() {
        let result: Vec<String> = shell_split("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_rewrite_clap_multi_args() {
        // This is the bug KuSh reported: `rtk rewrite ls -al` failed because
        // Clap rejected `-al` as an unknown flag. With trailing_var_arg + allow_hyphen_values,
        // multiple args are accepted and joined into a single command string.
        let cases = vec![
            vec!["rtk", "rewrite", "ls", "-al"],
            vec!["rtk", "rewrite", "git", "status"],
            vec!["rtk", "rewrite", "npm", "exec"],
            vec!["rtk", "rewrite", "cargo", "test"],
            vec!["rtk", "rewrite", "du", "-sh", "."],
            vec!["rtk", "rewrite", "head", "-50", "file.txt"],
        ];
        for args in &cases {
            let result = Cli::try_parse_from(args.iter());
            assert!(
                result.is_ok(),
                "rtk rewrite {:?} should parse (was failing before trailing_var_arg fix)",
                &args[2..]
            );
            if let Ok(cli) = result {
                match cli.command {
                    Commands::Rewrite { ref args } => {
                        assert!(args.len() >= 2, "rewrite args should capture all tokens");
                    }
                    _ => panic!("expected Rewrite command"),
                }
            }
        }
    }

    #[test]
    fn test_rewrite_clap_quoted_single_arg() {
        // Quoted form: `rtk rewrite "git status"` — single arg containing spaces
        let result = Cli::try_parse_from(["rtk", "rewrite", "git status"]);
        assert!(result.is_ok());
        if let Ok(cli) = result {
            match cli.command {
                Commands::Rewrite { ref args } => {
                    assert_eq!(args.len(), 1);
                    assert_eq!(args[0], "git status");
                }
                _ => panic!("expected Rewrite command"),
            }
        }
    }

    #[test]
    fn test_merge_filters_with_no_args() {
        let filters = vec![];
        let args = vec!["--depth=0".to_string(), "--no-verbose".to_string()];
        let expected_args = vec!["--depth=0", "--no-verbose"];
        assert_eq!(merge_pnpm_args(&filters, &args), expected_args);
    }

    #[test]
    fn test_merge_filters_with_args() {
        let filters = vec!["@app1".to_string(), "@app2".to_string()];
        let args = vec![
            "--filter=@app3".to_string(),
            "--depth=0".to_string(),
            "--no-verbose".to_string(),
        ];
        let expected_args = vec![
            "--filter=@app1",
            "--filter=@app2",
            "--filter=@app3",
            "--depth=0",
            "--no-verbose",
        ];
        assert_eq!(merge_pnpm_args(&filters, &args), expected_args);
    }

    #[test]
    fn test_merge_filters_with_no_args_os() {
        let filters = vec![];
        let args = vec![OsString::from("--depth=0")];
        let expected_args = vec![OsString::from("--depth=0")];
        assert_eq!(merge_pnpm_args_os(&filters, &args), expected_args);
    }

    #[test]
    fn test_merge_filters_with_args_os() {
        let filters = vec!["@app1".to_string()];
        let args = vec![OsString::from("--depth=0")];
        let expected_args = vec![
            OsString::from("--filter=@app1"),
            OsString::from("--depth=0"),
        ];
        assert_eq!(merge_pnpm_args_os(&filters, &args), expected_args);
    }

    #[test]
    fn test_pnpm_subcommand_with_filter() {
        let cli = Cli::try_parse_from([
            "rtk", "pnpm", "--filter", "@app1", "--filter", "@app2", "list", "--filter", "@app3",
            "--filter", "@app4", "--prod",
        ])
        .unwrap();
        match cli.command {
            Commands::Pnpm {
                filter,
                command: PnpmCommands::List { depth, args },
            } => {
                assert_eq!(depth, 0);
                assert_eq!(filter, vec!["@app1", "@app2"]);
                assert_eq!(
                    args,
                    vec!["--filter", "@app3", "--filter", "@app4", "--prod"]
                );
            }
            _ => panic!("Expected Pnpm List command"),
        }
    }

    #[test]
    fn test_git_push_u_flag_passes_through() {
        let cli = Cli::try_parse_from(["rtk", "git", "push", "-u", "origin", "my-branch"]).unwrap();
        assert!(
            !cli.ultra_compact,
            "-u on git push must NOT be consumed as --ultra-compact"
        );
        match cli.command {
            Commands::Git {
                command: GitCommands::Push { args },
                ..
            } => {
                assert!(
                    args.contains(&"-u".to_string()),
                    "-u must be forwarded to git push, got: {:?}",
                    args
                );
            }
            _ => panic!("Expected Git Push command"),
        }
    }

    #[test]
    fn test_pnpm_subcommand_with_short_filter() {
        // -F is the short form of --filter in pnpm
        let cli =
            Cli::try_parse_from(["rtk", "pnpm", "-F", "@app1", "-F", "@app2", "list"]).unwrap();
        match cli.command {
            Commands::Pnpm { filter, .. } => {
                assert_eq!(filter, vec!["@app1", "@app2"]);
            }
            _ => panic!("Expected Pnpm command"),
        }
    }

    #[test]
    fn test_pnpm_typecheck_without_filters() {
        let cli = Cli::try_parse_from([
            "rtk",
            "pnpm",
            "typecheck",
            "--filter",
            "@app3",
            "--filter",
            "@app4",
        ])
        .unwrap();
        match cli.command {
            Commands::Pnpm { filter, command } => {
                let warning = validate_pnpm_filters(&filter, &command);

                assert!(filter.is_empty());
                assert!(warning.is_none())
            }
            _ => panic!("Expected Pnpm Build command"),
        }
    }

    #[test]
    fn test_pnpm_typecheck_with_filters() {
        let cli = Cli::try_parse_from([
            "rtk",
            "pnpm",
            "--filter",
            "@app1",
            "--filter",
            "@app2",
            "typecheck",
            "--filter",
            "@app3",
            "--filter",
            "@app4",
        ])
        .unwrap();
        match cli.command {
            Commands::Pnpm { filter, command } => {
                let warning = validate_pnpm_filters(&filter, &command).unwrap();

                assert_eq!(filter, vec!["@app1", "@app2"]);
                assert_eq!(warning, "[rtk] warning: --filter is not yet supported for pnpm tsc, filters preceding the subcommand will be ignored")
            }
            _ => panic!("Expected Pnpm Build command"),
        }
    }

    #[test]
    fn test_ultra_compact_long_form_still_works() {
        let cli = Cli::try_parse_from(["rtk", "--ultra-compact", "git", "status"]).unwrap();
        assert!(
            cli.ultra_compact,
            "--ultra-compact long form must still enable ultra-compact mode"
        );
    }

    fn help_template_covers_all_commands() {
        let cmd = Cli::command();
        for sub in cmd.get_subcommands() {
            let name = sub.get_name();
            if name == "help" {
                continue;
            }
            assert!(
                HELP_TEMPLATE.contains(&format!("  {name}")),
                "Command '{name}' is missing from HELP_TEMPLATE — add it to the correct section",
            );
        }
    }
}

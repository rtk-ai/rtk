/// Canonical types for tool outputs
/// These provide a unified interface across different tool versions
use serde::{Deserialize, Serialize};

/// Test execution result (vitest, playwright, jest, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub duration_ms: Option<u64>,
    pub failures: Vec<TestFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFailure {
    pub test_name: String,
    pub file_path: String,
    pub error_message: String,
    pub stack_trace: Option<String>,
}

/// Linting result (eslint, biome, tsc, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResult {
    pub total_files: usize,
    pub files_with_issues: usize,
    pub total_issues: usize,
    pub errors: usize,
    pub warnings: usize,
    pub issues: Vec<LintIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintIssue {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub severity: LintSeverity,
    pub rule_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

/// Dependency state (pnpm, npm, cargo, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyState {
    pub total_packages: usize,
    pub outdated_count: usize,
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub wanted_version: Option<String>,
    pub dev_dependency: bool,
}

/// Build output (next, webpack, vite, cargo, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutput {
    pub success: bool,
    pub duration_ms: Option<u64>,
    pub warnings: usize,
    pub errors: usize,
    pub bundles: Vec<BundleInfo>,
    pub routes: Vec<RouteInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleInfo {
    pub name: String,
    pub size_bytes: u64,
    pub gzip_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteInfo {
    pub path: String,
    pub size_kb: f64,
    pub first_load_js_kb: Option<f64>,
}

/// Git operation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitResult {
    pub operation: String,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    pub commits: Vec<GitCommit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommit {
    pub hash: String,
    pub author: String,
    pub message: String,
    pub timestamp: Option<String>,
}

/// Generic command output (for tools without specific types)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub summary: Option<String>,
}

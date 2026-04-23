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

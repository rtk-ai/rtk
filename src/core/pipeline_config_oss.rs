//! Pipeline surface for the Apache-2.0 edition, which has no engine to ask.
//!
//! Every entry point returns the answer the full edition already returns when
//! no pipeline file is present, so the shell rewrite path is the one the CLI
//! has always taken. A `pipeline.toml` on disk is inert here, exactly as it is
//! for a build that predates the engine.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub enum ShellHookDecision {
    Deny { message: String, rule: String },
    Route { message: String, rule: String },
    Swap { command: String, rule: String },
    Answer { message: String, rule: String },
    Passthrough { rule: String },
    NoRewrite,
    Rewrite,
}

pub struct ReadHookDecision {
    pub message: String,
    pub rule: String,
    pub action: &'static str,
}

pub fn any_user_pipeline_files() -> bool {
    false
}

pub fn user_pipeline_path() -> Option<PathBuf> {
    None
}

pub fn set_call_purpose(_purpose: &str) {}

pub fn before_shell_decision(_command: &str) -> ShellHookDecision {
    ShellHookDecision::Rewrite
}

pub fn before_tool_decision(_subject: (), _facts: ()) -> ShellHookDecision {
    ShellHookDecision::Rewrite
}

pub fn before_read_decision(_path: &Path, _whole_file: bool) -> Option<ReadHookDecision> {
    None
}

pub fn read_status() -> Option<serde_json::Value> {
    None
}

pub fn project_pipeline_path() -> PathBuf {
    crate::core::user_dirs::in_working_dir(".rtk").join(crate::core::constants::PIPELINE_TOML)
}

pub fn project_pipelines_dir() -> PathBuf {
    crate::core::user_dirs::in_working_dir(".rtk").join("pipelines")
}

pub fn trust_summary(_content: &str) -> Result<Vec<String>, String> {
    Ok(vec![
        "pipeline files are inert in this build (Apache-2.0 edition)".to_string(),
    ])
}

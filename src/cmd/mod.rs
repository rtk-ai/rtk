//! RTK Command Engine - Hybrid Safe-Split Architecture
//!
//! This module provides:
//! - Quote-aware lexing for shell commands
//! - Native execution for simple chains
//! - Passthrough to /bin/sh for complex scripts
//! - Safety interception (rm -> trash, etc.)
//! - Token-optimized output filtering
//! - Hook protocol support (Claude/Gemini)

pub mod analysis;
pub mod builtins;
pub mod exec;
pub mod filters;
pub mod gemini_hook;
pub mod hook;
pub mod lexer;
pub mod predicates;
pub mod safety;
pub mod trash_cmd;

#[cfg(test)]
mod edge_cases;
#[cfg(test)]
pub(crate) mod test_helpers;

pub use exec::execute;
pub use hook::check_for_hook;

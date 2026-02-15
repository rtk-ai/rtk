//! RTK command interceptor — safety checks and token-optimized execution.
//!
//! This module provides:
//! - Quote-aware lexing for shell commands
//! - Native execution for simple chains
//! - Passthrough to /bin/sh for complex scripts
//! - Safety interception (rm -> trash, etc.)
//! - Token-optimized output filtering
//! - Hook protocol support (Claude/Gemini)

pub(crate) mod analysis;
pub(crate) mod builtins;
pub mod exec;
pub(crate) mod filters;
pub mod gemini_hook;
pub mod hook;
pub(crate) mod lexer;
pub(crate) mod predicates;
pub(crate) mod safety;
pub(crate) mod trash_cmd;

#[cfg(test)]
pub(crate) mod test_helpers;

pub use exec::execute;
pub use hook::check_for_hook;

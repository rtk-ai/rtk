//! Command execution subsystem for RTK hook integration.
//!
//! This module provides the core hook engine that powers `rtk hook claude`.
//! It handles chained command rewriting, native command execution, and output filtering.

// Analysis and lexing (no external deps)
pub(crate) mod analysis;
pub(crate) mod lexer;

// Safety engine (depends on config::rules)
pub(crate) mod safety;

// Trash command (depends on trash crate)
pub(crate) mod trash_cmd;

// Predicates and utilities (no external deps)
pub(crate) mod predicates;

// Builtins (depends on predicates)
pub(crate) mod builtins;

// Filters (depends on crate::utils)
pub(crate) mod filters;

// Exec (depends on analysis, builtins, filters, lexer)
pub mod exec;

// Hook logic (depends on analysis, lexer)
pub mod hook;

// Claude hook protocol (depends on hook)
pub mod claude_hook;

#[cfg(test)]
pub(crate) mod test_helpers;

// Public exports
pub use exec::execute;
pub use hook::check_for_hook;

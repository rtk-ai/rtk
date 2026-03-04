//! Command execution subsystem for RTK hook integration.
//!
//! This module provides the core hook engine that powers `rtk hook claude`.
//! It handles chained command rewriting, native command execution, and output filtering.

// Analysis and lexing (no external deps)
pub(crate) mod analysis;
pub(crate) mod lexer;

// Predicates and utilities (no external deps)
pub(crate) mod predicates;

// Builtins (depends on predicates)
pub(crate) mod builtins;

// Filters (depends on crate::utils)
pub(crate) mod filters;

// Exec (depends on analysis, builtins, filters, lexer)
pub mod exec;

// Hook logic + LLM protocol adapters (hook/mod.rs, hook/claude.rs)
pub mod hook;

// Safety wrapper for rm→trash (renamed from trash_cmd.rs; wired up in main branch)
// pub(crate) mod trash;  // not declared here to avoid name collision with external `trash` crate

#[cfg(test)]
pub(crate) mod test_helpers;

// Public exports
pub use exec::execute;
pub use hook::check_for_hook;

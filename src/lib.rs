//! Library crate for rtk.
//!
//! This crate exists so that code shared between the `rtk` binary (the
//! Clap-based CLI, see `src/main.rs`) and the `rtk-shell` binary (the
//! persistent/one-shot shell entry point, see `src/bin/rtk_shell.rs`) lives
//! in exactly one place instead of being duplicated or gated behind
//! `#[path]` hacks.
//!
//! `main.rs` still owns the full `Commands` enum and CLI wiring; it depends
//! on this crate the same way an external consumer would (`use rtk::...`).
//! Everything under these modules keeps working exactly as before — this
//! split only changes *where* the module tree is declared, not its shape.

pub mod analytics;
pub mod cmds;
pub mod core;
pub mod discover;
pub mod hooks;
pub mod learn;
pub mod parser;
pub mod shell;

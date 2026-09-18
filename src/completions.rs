//! Shell completion script generation (bash, zsh, fish, elvish, powershell).
//!
//! Completions are generated at runtime: `rtk completions <shell>` prints the
//! script for the requested shell to stdout so the user can source it or
//! redirect it into their shell's completion directory.

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::Shell;

/// Write a completion script for `shell` to stdout.
pub fn generate(shell: Shell) -> Result<()> {
    let mut cmd = crate::Cli::command();
    clap_complete::generate(shell, &mut cmd, "rtk", &mut std::io::stdout());
    Ok(())
}

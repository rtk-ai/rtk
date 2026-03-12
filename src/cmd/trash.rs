//! Built-in trash - mirrors rm behavior: silent on success, error on failure.
//!
//! On macOS, uses NSFileManager instead of Finder to avoid the trash sound effect.
//! This is a deliberate tradeoff: we lose the "Put Back" option in Finder but gain
//! silent operation which is important for background CLI usage.

use anyhow::Result;
use std::path::Path;

#[cfg(target_os = "macos")]
use trash::macos::TrashContextExtMacos;

pub fn execute(paths: &[String]) -> Result<bool> {
    let expanded: Vec<String> = paths
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| super::predicates::expand_tilde(p))
        .collect();

    if expanded.is_empty() {
        eprintln!("trash: no paths specified");
        return Ok(false);
    }

    let (existing, missing): (Vec<_>, Vec<_>) =
        expanded.iter().partition(|p| Path::new(p).exists());

    // Report missing like rm does
    for p in &missing {
        eprintln!("trash: cannot remove '{}': No such path", p);
    }

    if existing.is_empty() {
        return Ok(false);
    }

    let refs: Vec<&str> = existing.iter().map(|s| s.as_str()).collect();

    // Use a custom TrashContext on macOS to avoid the trash sound
    // The default uses Finder which plays the system trash sound
    #[cfg(target_os = "macos")]
    {
        let mut ctx = trash::TrashContext::new();
        ctx.set_delete_method(trash::macos::DeleteMethod::NsFileManager);
        match ctx.delete_all(&refs) {
            Ok(_) => Ok(true),
            Err(e) => {
                eprintln!("trash: {}", e);
                Ok(false)
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    match trash::delete_all(&refs) {
        Ok(_) => Ok(true),
        Err(e) => {
            eprintln!("trash: {}", e);
            Ok(false)
        }
    }
}

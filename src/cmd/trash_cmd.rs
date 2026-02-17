//! Built-in trash - mirrors rm behavior: silent on success, error on failure.

use anyhow::Result;
use std::path::Path;

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
    match trash::delete_all(&refs) {
        Ok(_) => Ok(true),
        Err(e) => {
            eprintln!("trash: {}", e);
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rtk_{}", name));
        fs::write(&p, "x").unwrap();
        p
    }
    fn rm(p: &PathBuf) {
        let _ = fs::remove_file(p);
    }

    #[test]
    fn t_empty() {
        assert!(!execute(&[]).unwrap());
    }
    #[test]
    fn t_missing() {
        assert!(!execute(&["/nope".into()]).unwrap());
    }
    #[test]
    fn t_single() {
        let p = tmp("s");
        assert!(execute(&[p.to_string_lossy().into()]).unwrap());
        rm(&p);
    }
    #[test]
    fn t_multi() {
        let (a, b) = (tmp("a"), tmp("b"));
        assert!(execute(&[a.to_string_lossy().into(), b.to_string_lossy().into()]).unwrap());
        rm(&a);
        rm(&b);
    }
}

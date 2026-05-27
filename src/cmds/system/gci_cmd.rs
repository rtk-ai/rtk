//! PowerShell Get-ChildItem / gci / dir compatible (subset) file listing with compact output.

use crate::core::tracking;
use anyhow::Result;
use ignore::WalkBuilder;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GciKind {
    File,
    Directory,
    Any,
}

#[derive(Debug)]
pub struct GciArgs {
    pub path: PathBuf,
    pub recurse: bool,
    pub force: bool,
    pub kind: GciKind,
    pub filter: Option<String>,
    pub include: Vec<String>,
    pub max: usize,
    pub select_full_name: bool,
    pub select_last_write_time: bool,
    pub select_length: bool,
}

impl Default for GciArgs {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
            recurse: false,
            force: false,
            kind: GciKind::Any,
            filter: None,
            include: Vec::new(),
            max: 50,
            select_full_name: true,
            select_last_write_time: false,
            select_length: false,
        }
    }
}

pub fn run(args: &GciArgs, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("gci: {} (recurse={})", args.path.display(), args.recurse);
    }

    let mut builder = WalkBuilder::new(&args.path);
    builder.git_ignore(true).git_exclude(true).hidden(!args.force);
    if !args.recurse {
        builder.max_depth(Some(1));
    }

    let filter = args.filter.as_deref();
    let include = &args.include;

    let mut matches: Vec<PathBuf> = Vec::new();
    for dent in builder.build() {
        let dent = match dent {
            Ok(d) => d,
            Err(_) => continue,
        };
        let p = dent.path();
        if p == Path::new("") {
            continue;
        }

        let ft = match dent.file_type() {
            Some(t) => t,
            None => continue,
        };
        match args.kind {
            GciKind::File if !ft.is_file() => continue,
            GciKind::Directory if !ft.is_dir() => continue,
            _ => {}
        }

        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if let Some(f) = filter {
            if !glob_match(f, name) {
                continue;
            }
        }

        if !include.is_empty() && !include.iter().any(|pat| glob_match(pat, name)) {
            continue;
        }

        matches.push(p.to_path_buf());
    }

    matches.sort();
    let total = matches.len();

    let mut out = String::new();
    out.push_str(&format!("{} matches\n\n", total));

    let shown = std::cmp::min(total, args.max);
    for p in matches.iter().take(shown) {
        if args.select_last_write_time || args.select_length {
            let meta = fs::metadata(p).ok();
            let len = meta.as_ref().map(|m| m.len());
            let mtime = meta.as_ref().and_then(|m| m.modified().ok());

            let mut parts = Vec::new();
            if args.select_full_name {
                parts.push(p.display().to_string());
            }
            if args.select_length {
                parts.push(format!(
                    "len={}",
                    len.map(|v| v.to_string()).unwrap_or_else(|| "?".into())
                ));
            }
            if args.select_last_write_time {
                parts.push(format!(
                    "mtime={}",
                    mtime.map(format_system_time).unwrap_or_else(|| "?".into())
                ));
            }
            out.push_str(&parts.join("  "));
            out.push('\n');
        } else {
            out.push_str(&p.display().to_string());
            out.push('\n');
        }
    }

    if total > shown {
        out.push_str(&format!("[+{} more]\n", total - shown));
    }

    print!("{}", out);
    timer.track(
        &format!("gci {}", args.path.display()),
        "rtk gci",
        "",
        &out,
    );
    Ok(())
}

pub fn parse_select_list(spec: &str, out: &mut GciArgs) {
    // Accept: "FullName,LastWriteTime,Length" (powershell-ish).
    for raw in spec.split(',') {
        let s = raw.trim().to_ascii_lowercase();
        match s.as_str() {
            "fullname" => out.select_full_name = true,
            "lastwritetime" => out.select_last_write_time = true,
            "length" => out.select_length = true,
            _ => {}
        }
    }
}

fn format_system_time(t: SystemTime) -> String {
    // Avoid chrono dependency; emit seconds since epoch for compactness.
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}s", d.as_secs()),
        Err(_) => "?".into(),
    }
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let pat = pattern.to_ascii_lowercase();
    let nm = name.to_ascii_lowercase();
    glob_match_inner(pat.as_bytes(), nm.as_bytes())
}

fn glob_match_inner(pat: &[u8], name: &[u8]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            glob_match_inner(&pat[1..], name)
                || (!name.is_empty() && glob_match_inner(pat, &name[1..]))
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pat[1..], &name[1..]),
        (Some(&p), Some(&n)) if p == n => glob_match_inner(&pat[1..], &name[1..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match_case_insensitive() {
        assert!(glob_match("*.CPP", "api_win.cpp"));
        assert!(glob_match("API_WIN.OBJ", "api_win.obj"));
    }

    #[test]
    fn test_glob_match_wildcards() {
        assert!(glob_match("a?c.txt", "abc.txt"));
        assert!(glob_match("a*c.txt", "abbbbbc.txt"));
        assert!(!glob_match("a?c.txt", "ac.txt"));
    }
}

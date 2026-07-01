//! PowerShell command shims that route common inspection commands into RTK filters.

use crate::cmds::system::{ls, read, search, wc_cmd};
use crate::core::filter::FilterLevel;
use crate::core::utils::tool_exists;
use anyhow::{Context, Result};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn is_powershell_flag(arg: &str, name: &str) -> bool {
    arg.eq_ignore_ascii_case(name)
}

pub fn run_read_command(
    files: &[PathBuf],
    level: FilterLevel,
    max_lines: Option<usize>,
    tail_lines: Option<usize>,
    line_numbers: bool,
    verbose: u8,
) -> Result<i32> {
    let mut had_error = false;
    let mut stdin_seen = false;
    for file in files {
        let result = if file == Path::new("-") {
            if stdin_seen {
                eprintln!("rtk: warning: stdin specified more than once");
                continue;
            }
            stdin_seen = true;
            read::run_stdin(level, max_lines, tail_lines, line_numbers, verbose)
        } else {
            read::run(file, level, max_lines, tail_lines, line_numbers, verbose)
        };
        if let Err(e) = result {
            eprintln!("cat: {}: {}", file.display(), e.root_cause());
            had_error = true;
        }
    }
    Ok(if had_error { 1 } else { 0 })
}

pub fn run_get_child_item(args: &[String], verbose: u8) -> Result<i32> {
    let mut translated = Vec::new();
    let mut recurse = false;
    let mut show_all = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            x if is_powershell_flag(x, "-Force") => {
                translated.push("-a".to_string());
                show_all = true;
            }
            x if is_powershell_flag(x, "-Recurse") => {
                translated.push("-R".to_string());
                recurse = true;
            }
            x if is_powershell_flag(x, "-Path") || is_powershell_flag(x, "-LiteralPath") => {
                let value = args
                    .get(i + 1)
                    .context("Get-ChildItem: missing value for -Path/-LiteralPath")?;
                translated.push(value.clone());
                i += 1;
            }
            x if x.starts_with('-') => {
                anyhow::bail!("Get-ChildItem: unsupported PowerShell flag: {}", x);
            }
            other => translated.push(other.to_string()),
        }
        i += 1;
    }

    if !cfg!(windows) {
        return ls::run(&translated, verbose);
    }

    let mut paths: Vec<PathBuf> = translated
        .into_iter()
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }

    for (idx, root) in paths.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        if paths.len() > 1 {
            println!("{}:", root.display());
        }
        render_get_child_item_path(root, recurse, show_all)?;
    }
    Ok(0)
}

fn render_get_child_item_path(root: &Path, recurse: bool, show_all: bool) -> Result<()> {
    let root_meta = fs::metadata(root)
        .with_context(|| format!("Get-ChildItem: failed to access {}", root.display()))?;

    let walker = if root_meta.is_dir() {
        if recurse {
            WalkDir::new(root).min_depth(1)
        } else {
            WalkDir::new(root).min_depth(1).max_depth(1)
        }
    } else {
        WalkDir::new(root).min_depth(0).max_depth(0)
    };

    for entry in walker.into_iter().filter_map(Result::ok) {
        let path = entry.path();
        let name = if recurse && root_meta.is_dir() {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        } else {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string())
        };

        let leaf_hidden = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'));
        if !show_all && leaf_hidden {
            continue;
        }

        let md = entry
            .metadata()
            .with_context(|| format!("Get-ChildItem: failed to stat {}", path.display()))?;
        if md.is_dir() {
            println!("{}/", name);
        } else {
            println!("{} {}B", name, md.len());
        }
    }
    Ok(())
}

pub fn run_get_content(args: &[String], verbose: u8) -> Result<i32> {
    let mut files = Vec::<PathBuf>::new();
    let mut max_lines = None;
    let mut tail_lines = None;
    let line_numbers = false;
    let mut level = FilterLevel::None;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            x if is_powershell_flag(x, "-Path") || is_powershell_flag(x, "-LiteralPath") => {
                let value = args
                    .get(i + 1)
                    .context("Get-Content: missing value for -Path/-LiteralPath")?;
                files.push(PathBuf::from(value));
                i += 1;
            }
            x if is_powershell_flag(x, "-Tail") => {
                let value = args
                    .get(i + 1)
                    .context("Get-Content: missing value for -Tail")?;
                tail_lines = Some(value.parse().context("Get-Content: invalid -Tail value")?);
                i += 1;
            }
            x if is_powershell_flag(x, "-TotalCount") => {
                let value = args
                    .get(i + 1)
                    .context("Get-Content: missing value for -TotalCount")?;
                max_lines = Some(
                    value
                        .parse()
                        .context("Get-Content: invalid -TotalCount value")?,
                );
                i += 1;
            }
            x if is_powershell_flag(x, "-Raw") => {
                level = FilterLevel::None;
            }
            x if is_powershell_flag(x, "-ReadCount")
                || is_powershell_flag(x, "-Encoding")
                || is_powershell_flag(x, "-Wait")
                || is_powershell_flag(x, "-AsByteStream") =>
            {
                anyhow::bail!("Get-Content: unsupported PowerShell flag: {}", x);
            }
            x if x.starts_with('-') => {
                anyhow::bail!("Get-Content: unsupported PowerShell flag: {}", x);
            }
            other => files.push(PathBuf::from(other)),
        }
        i += 1;
    }

    if files.is_empty() {
        return read::run_stdin(level, max_lines, tail_lines, line_numbers, verbose).map(|_| 0);
    }

    run_read_command(&files, level, max_lines, tail_lines, line_numbers, verbose)
}

pub fn run_select_string(args: &[String], verbose: u8) -> Result<i32> {
    let mut translated = Vec::<String>::new();
    let mut pattern: Option<String> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut case_sensitive = false;
    let mut simple_match = false;
    let mut not_match = false;
    let mut list_only = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            x if is_powershell_flag(x, "-Pattern") => {
                let value = args
                    .get(i + 1)
                    .context("Select-String: missing value for -Pattern")?;
                pattern = Some(value.clone());
                i += 1;
            }
            x if is_powershell_flag(x, "-Path") || is_powershell_flag(x, "-LiteralPath") => {
                let value = args
                    .get(i + 1)
                    .context("Select-String: missing value for -Path/-LiteralPath")?;
                paths.push(value.clone());
                i += 1;
            }
            x if is_powershell_flag(x, "-CaseSensitive") => case_sensitive = true,
            x if is_powershell_flag(x, "-SimpleMatch") => simple_match = true,
            x if is_powershell_flag(x, "-NotMatch") => not_match = true,
            x if is_powershell_flag(x, "-List") => list_only = true,
            x if is_powershell_flag(x, "-Context")
                || is_powershell_flag(x, "-AllMatches")
                || is_powershell_flag(x, "-Quiet")
                || is_powershell_flag(x, "-Encoding")
                || is_powershell_flag(x, "-Include")
                || is_powershell_flag(x, "-Exclude")
                || is_powershell_flag(x, "-Raw") =>
            {
                anyhow::bail!("Select-String: unsupported PowerShell flag: {}", x);
            }
            x if x.starts_with('-') => {
                anyhow::bail!("Select-String: unsupported PowerShell flag: {}", x);
            }
            other => {
                if pattern.is_none() {
                    pattern = Some(other.to_string());
                } else {
                    paths.push(other.to_string());
                }
            }
        }
        i += 1;
    }

    let pattern = pattern.context("Select-String: missing search pattern")?;
    if !case_sensitive {
        translated.push("-i".to_string());
    }
    if simple_match {
        translated.push("-F".to_string());
    }
    if not_match {
        translated.push("-v".to_string());
    }
    if list_only {
        translated.push("-l".to_string());
    }
    translated.push(pattern);
    translated.extend(paths);

    let engine = if tool_exists("rg") {
        search::Engine::Rg
    } else {
        search::Engine::Grep
    };
    search::run(engine, 80, 200, false, &translated, verbose)
}

pub fn run_measure_object(args: &[String], verbose: u8) -> Result<i32> {
    let mut translated = Vec::<String>::new();
    let mut saw_mode = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            x if is_powershell_flag(x, "-Line") => {
                translated.push("-l".to_string());
                saw_mode = true;
            }
            x if is_powershell_flag(x, "-Word") => {
                translated.push("-w".to_string());
                saw_mode = true;
            }
            x if is_powershell_flag(x, "-Character") || is_powershell_flag(x, "-Chars") => {
                translated.push("-m".to_string());
                saw_mode = true;
            }
            x if is_powershell_flag(x, "-InputObject")
                || is_powershell_flag(x, "-Sum")
                || is_powershell_flag(x, "-Average")
                || is_powershell_flag(x, "-Maximum")
                || is_powershell_flag(x, "-Minimum")
                || is_powershell_flag(x, "-Property") =>
            {
                anyhow::bail!("Measure-Object: unsupported PowerShell flag: {}", x);
            }
            x if x.starts_with('-') => {
                anyhow::bail!("Measure-Object: unsupported PowerShell flag: {}", x);
            }
            other => translated.push(other.to_string()),
        }
        i += 1;
    }

    if !saw_mode {
        anyhow::bail!("Measure-Object: specify at least one of -Line, -Word, or -Character");
    }

    if !cfg!(windows) {
        return wc_cmd::run(&translated, verbose);
    }

    let mut line = false;
    let mut word = false;
    let mut character = false;
    let mut inputs: Vec<PathBuf> = Vec::new();
    for arg in &translated {
        match arg.as_str() {
            "-l" => line = true,
            "-w" => word = true,
            "-m" => character = true,
            other => inputs.push(PathBuf::from(other)),
        }
    }

    let mut content = String::new();
    if inputs.is_empty() {
        std::io::stdin()
            .lock()
            .read_to_string(&mut content)
            .context("Measure-Object: failed to read stdin")?;
    } else {
        for file in &inputs {
            content.push_str(
                &fs::read_to_string(file).with_context(|| {
                    format!("Measure-Object: failed to read {}", file.display())
                })?,
            );
            content.push('\n');
        }
    }

    let lines = if line { content.lines().count() } else { 0 };
    let words = if word {
        content.split_whitespace().count()
    } else {
        0
    };
    let chars = if character {
        content.chars().count()
    } else {
        0
    };

    match (line, word, character) {
        (true, false, false) => println!("{}", lines),
        (false, true, false) => println!("{}", words),
        (false, false, true) => println!("{}", chars),
        _ => println!("{} {} {}", lines, words, chars),
    }
    Ok(0)
}

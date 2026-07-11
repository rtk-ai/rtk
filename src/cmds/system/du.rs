//! Disk usage summary with Windows-native compact output.

use anyhow::{anyhow, Result};
#[cfg(not(target_os = "windows"))]
use crate::core::utils::{exit_code_from_status, resolved_command};
#[cfg(any(target_os = "windows", test))]
use std::io::Write;
#[cfg(any(target_os = "windows", test))]
use std::path::Path;
use std::path::PathBuf;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    #[cfg(target_os = "windows")]
    {
        run_native(args, verbose)
    }

    #[cfg(not(target_os = "windows"))]
    {
        run_external(args, verbose)
    }
}

#[cfg(not(target_os = "windows"))]
fn run_external(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("Running: du {}", args.join(" "));
    }
    let status = resolved_command("du").args(args).status()?;
    Ok(exit_code_from_status(&status, "du"))
}

#[cfg(target_os = "windows")]
fn run_native(args: &[String], verbose: u8) -> Result<i32> {
    if args.iter().any(|a| a == "--help") {
        print_help();
        return Ok(0);
    }

    let options = match parse_args(args) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("rtk du: {err}");
            return Ok(2);
        }
    };

    if verbose > 0 {
        eprintln!("Running native du {}", args.join(" "));
    }

    let stderr = std::io::stderr();
    let mut errors = stderr.lock();
    match native_du_output(&options, &mut errors) {
        Ok(output) => {
            print!("{output}");
            Ok(0)
        }
        Err(err) => {
            eprintln!("rtk du: {err}");
            Ok(2)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DuOptions {
    human: bool,
    summarize: bool,
    max_depth: Option<usize>,
    paths: Vec<PathBuf>,
}

impl Default for DuOptions {
    fn default() -> Self {
        Self {
            human: false,
            summarize: false,
            max_depth: None,
            paths: vec![PathBuf::from(".")],
        }
    }
}

fn parse_args(args: &[String]) -> Result<DuOptions> {
    let mut options = DuOptions {
        paths: Vec::new(),
        ..DuOptions::default()
    };
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--help" => {
                i += 1;
            }
            "--summarize" => {
                options.summarize = true;
                i += 1;
            }
            "--human-readable" => {
                options.human = true;
                i += 1;
            }
            "--max-depth" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--max-depth requires a value"))?;
                set_depth(&mut options, value)?;
                i += 2;
            }
            _ if arg.starts_with("--max-depth=") => {
                set_depth(&mut options, &arg["--max-depth=".len()..])?;
                i += 1;
            }
            _ if arg.starts_with("--") => {
                return Err(anyhow!(
                    "unsupported du flag '{arg}' on Windows native path; use rtk proxy du ..."
                ));
            }
            _ if arg.starts_with('-') && arg != "-" => {
                let cluster = arg.trim_start_matches('-');
                if cluster.is_empty() {
                    options.paths.push(PathBuf::from(arg));
                    i += 1;
                    continue;
                }

                let mut chars = cluster.chars().peekable();
                while let Some(flag) = chars.next() {
                    match flag {
                        's' => options.summarize = true,
                        'h' => options.human = true,
                        'd' => {
                            let rest: String = chars.collect();
                            if rest.is_empty() {
                                let value = args
                                    .get(i + 1)
                                    .ok_or_else(|| anyhow!("-d requires a value"))?;
                                set_depth(&mut options, value)?;
                                i += 1;
                            } else {
                                set_depth(&mut options, &rest)?;
                            }
                            break;
                        }
                        other => {
                            return Err(anyhow!(
                                "unsupported du flag '-{other}' on Windows native path; use rtk proxy du ..."
                            ));
                        }
                    }
                }
                i += 1;
            }
            _ => {
                options.paths.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    if options.paths.is_empty() {
        options.paths.push(PathBuf::from("."));
    }

    Ok(options)
}

fn set_depth(options: &mut DuOptions, raw: &str) -> Result<()> {
    if options.max_depth.is_some() {
        return Err(anyhow!("duplicate max depth"));
    }
    if raw.starts_with('-') {
        return Err(anyhow!("max depth must be non-negative"));
    }
    let depth = raw
        .parse::<usize>()
        .map_err(|_| anyhow!("invalid max depth '{raw}'"))?;
    options.max_depth = Some(depth);
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn native_du_output<W: Write>(options: &DuOptions, errors: &mut W) -> Result<String> {
    let mut output = String::new();
    for path in &options.paths {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                report_access_error(errors, path, &err)?;
                continue;
            }
        };
        let mut rows = Vec::new();
        let size = collect_path_size(path, &metadata, 0, options, &mut rows, errors)?;
        if options.summarize || options.max_depth.is_none() {
            output.push_str(&format!(
                "{}\t{}\n",
                format_du_size(size, options.human),
                path.display()
            ));
        } else {
            rows.sort_by(|a, b| a.1.cmp(&b.1));
            for (_, row_path, row_size) in rows {
                output.push_str(&format!(
                    "{}\t{}\n",
                    format_du_size(row_size, options.human),
                    row_path.display()
                ));
            }
        }
    }
    Ok(output)
}

#[cfg(any(target_os = "windows", test))]
fn collect_path_size<W: Write>(
    path: &Path,
    metadata: &std::fs::Metadata,
    depth: usize,
    options: &DuOptions,
    rows: &mut Vec<(usize, PathBuf, u64)>,
    errors: &mut W,
) -> Result<u64> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        let size = metadata.len();
        if options.max_depth.is_some_and(|max| depth <= max) {
            rows.push((depth, path.to_path_buf(), size));
        }
        return Ok(size);
    }

    let mut total = 0u64;
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => {
            report_access_error(errors, path, &err)?;
            return Ok(0);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                report_access_error(errors, path, &err)?;
                continue;
            }
        };
        let child_path = entry.path();
        let child_metadata = match child_path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                report_access_error(errors, &child_path, &err)?;
                continue;
            }
        };
        if child_metadata.file_type().is_symlink() {
            continue;
        }
        let child_size = collect_path_size(
            &child_path,
            &child_metadata,
            depth + 1,
            options,
            rows,
            errors,
        )?;
        total = total.saturating_add(child_size);
    }

    if options.max_depth.is_some_and(|max| depth <= max) {
        rows.push((depth, path.to_path_buf(), total));
    }
    Ok(total)
}

#[cfg(any(target_os = "windows", test))]
fn report_access_error<W: Write>(errors: &mut W, path: &Path, err: &std::io::Error) -> std::io::Result<()> {
    writeln!(errors, "rtk du: cannot access {}: {err}", path.display())
}

#[cfg(target_os = "windows")]
fn print_help() {
    println!(
        "Disk usage summary with compact output (native Windows)\n\n\
Usage: rtk du [OPTIONS] [PATHS]...\n\n\
Options:\n  -s, --summarize       show total per path\n  -h, --human-readable  compact sizes such as 1.2G\n  -d, --max-depth N     limit traversal depth\n      --help            print help\n\n\
Does not follow symlink/junction targets. Use `rtk proxy du ...` for native du semantics."
    );
}

fn format_du_size(bytes: u64, human: bool) -> String {
    if !human {
        return bytes.to_string();
    }
    compact_size(bytes)
}

fn compact_size(bytes: u64) -> String {
    const K: f64 = 1024.0;
    const M: f64 = K * 1024.0;
    const G: f64 = M * 1024.0;
    const T: f64 = G * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= T {
        format!("{:.1}T", bytes_f / T)
    } else if bytes_f >= G {
        format!("{:.1}G", bytes_f / G)
    } else if bytes_f >= M {
        format!("{:.1}M", bytes_f / M)
    } else if bytes_f >= K {
        format!("{:.1}K", bytes_f / K)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_du_depth_forms() {
        let args = vec!["-sh".to_string(), "-d1".to_string(), "target".to_string()];
        let options = parse_args(&args).unwrap();
        assert!(options.human);
        assert!(options.summarize);
        assert_eq!(options.max_depth, Some(1));
        assert_eq!(options.paths, vec![PathBuf::from("target")]);
    }

    #[test]
    fn test_parse_du_rejects_duplicate_depth() {
        let args = vec![
            "-d".to_string(),
            "1".to_string(),
            "--max-depth=2".to_string(),
        ];
        assert!(parse_args(&args).is_err());
    }

    #[test]
    fn test_compact_size_style() {
        assert_eq!(compact_size(978), "978B");
        assert_eq!(compact_size(1234), "1.2K");
        assert_eq!(compact_size(1_234_567_890), "1.1G");
    }

    #[test]
    fn test_parse_du_rejects_negative_depth() {
        let args = vec!["-d".to_string(), "-1".to_string()];
        assert!(parse_args(&args).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_native_du_output_summarizes_file_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, b"hello").unwrap();

        let options = DuOptions {
            paths: vec![file.clone()],
            ..DuOptions::default()
        };
        let mut errors = Vec::new();
        let output = native_du_output(&options, &mut errors).unwrap();

        assert_eq!(output, format!("5\t{}\n", file.display()));
        assert!(errors.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_native_du_output_max_depth_lists_root_and_direct_children() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        std::fs::write(child_dir.join("nested.txt"), b"hello").unwrap();
        let direct_file = dir.path().join("direct.txt");
        std::fs::write(&direct_file, b"abc").unwrap();

        let options = DuOptions {
            max_depth: Some(1),
            paths: vec![dir.path().to_path_buf()],
            ..DuOptions::default()
        };
        let mut errors = Vec::new();
        let output = native_du_output(&options, &mut errors).unwrap();

        assert!(output.contains(&format!("8\t{}", dir.path().display())));
        assert!(output.contains(&format!("5\t{}", child_dir.display())));
        assert!(output.contains(&format!("3\t{}", direct_file.display())));
        assert!(!output.contains("nested.txt"));
        assert!(errors.is_empty());
    }

    #[test]
    fn test_native_du_skips_missing_paths_without_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let options = DuOptions {
            paths: vec![dir.path().join("missing")],
            ..DuOptions::default()
        };
        let mut errors = Vec::new();

        let output = native_du_output(&options, &mut errors).unwrap();

        assert!(output.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn test_report_access_error_uses_the_rtk_du_prefix() {
        let path = PathBuf::from("blocked");
        let mut errors = Vec::new();

        report_access_error(
            &mut errors,
            &path,
            &std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied"),
        )
        .unwrap();

        assert_eq!(
            String::from_utf8(errors).unwrap(),
            "rtk du: cannot access blocked: denied\n"
        );
    }
}

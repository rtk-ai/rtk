use super::constants::NOISE_DIRS;
use super::ls::{self, LsRecord, LsRecordType, FormatOptions};

use anyhow::Result;
use colored::Colorize; 
use std::io::IsTerminal;
use std::path::Path;
use std::time::UNIX_EPOCH;


pub fn generate_mock_raw_output(records: &[LsRecord]) -> String {
    let mut out = String::new();
    let total_blocks = records.iter().map(|r| (r.size + 4095) / 4096 * 4).sum::<u64>();
    out.push_str(&format!("total {}\n", total_blocks));

    for r in records {
        let type_char = match r.file_type {
            LsRecordType::DIRECTORY => 'd',
            LsRecordType::SYMBOLINK => 'l',
            _ => '-',
        };
        let perms = match &r.octal_permissions {
            Some(oct) => {
                let mut p = String::new();
                for c in oct.chars() {
                    let val = c.to_digit(8).unwrap_or(0);
                    p.push(if val & 4 != 0 { 'r' } else { '-' });
                    p.push(if val & 2 != 0 { 'w' } else { '-' });
                    p.push(if val & 1 != 0 { 'x' } else { '-' });
                }
                p
            }
            None => "rwxr-xr-x".to_string(),
        };

        out.push_str(&format!(
            "{}{} 1 user staff {} Jan 1 12:00 {}\n",
            type_char, perms, r.size, r.name
        ));
    }
    out
}

fn is_file_hidden(name: &str, metadata: &std::fs::Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        (metadata.file_attributes() & 0x2) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Fetches file information from the filesystem using native Rust std::fs.
pub fn fetch_entries(paths: &[String], show_all: bool) -> Result<Vec<LsRecord>> {
    let mut records = Vec::new();
    let targets: Vec<String> = if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths.to_vec()
    };

    for path_str in &targets {
        let path = Path::new(path_str);
        if !path.exists() {
            eprintln!("rtk: {}: No such file or directory", path_str);
            continue;
        }

        let metadata = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if metadata.is_dir() {
            let resolved_path = match dunce::canonicalize(path) {
                Ok(p) => p,
                Err(_) => path.to_path_buf(),
            };

            for entry_res in std::fs::read_dir(&resolved_path)? {
                let entry = match entry_res {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let mut name = entry.file_name().to_string_lossy().to_string();

                if name == "." || name == ".." {
                    continue;
                }

                let meta = entry.metadata().or_else(|_| std::fs::symlink_metadata(entry.path())).ok();
                if let Some(ref m) = meta {
                    if !show_all && (is_file_hidden(&name, m) || NOISE_DIRS.iter().any(|noise| name == *noise)) {
                        continue;
                    }
                }

                if let Ok(file_type) = entry.file_type() {
                    let ls_file_type = if file_type.is_symlink() {
                        if let Ok(target) = std::fs::read_link(entry.path()) {
                            name = format!("{} -> {}", name, target.to_string_lossy());
                        }
                        LsRecordType::SYMBOLINK
                    } else if file_type.is_dir() {
                        LsRecordType::DIRECTORY
                    } else {
                        LsRecordType::FILE
                    };

                    let (size, timestamp, is_readonly) = if let Some(ref m) = meta {
                        let ts = m.modified()
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as u64)
                            .unwrap_or(0);
                        (m.len(), Some(ts), m.permissions().readonly())
                    } else {
                        (0, None, false)
                    };

                    let octal_permissions = if is_readonly {
                        if ls_file_type == LsRecordType::DIRECTORY { "555" } else { "444" }
                    } else {
                        if ls_file_type == LsRecordType::DIRECTORY { "755" } else { "644" }
                    }.to_string();

                    records.push(LsRecord {
                        extension: ls::get_extension(&name),
                        file_type: ls_file_type,
                        size,
                        name,
                        timestamp,
                        octal_permissions: Some(octal_permissions),
                    });
                }
            }
        } else {
            let mut name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
                
            let ls_file_type = if metadata.is_symlink() {
                if let Ok(target) = std::fs::read_link(path) {
                    name = format!("{} -> {}", name, target.to_string_lossy());
                }
                LsRecordType::SYMBOLINK
            } else {
                LsRecordType::FILE
            };

            let timestamp = Some(
                metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as u64)
                    .unwrap_or(0),
            );

            let is_readonly = metadata.permissions().readonly();
            let octal_permissions = if is_readonly {
                if ls_file_type == LsRecordType::DIRECTORY { "555" } else { "444" }
            } else {
                if ls_file_type == LsRecordType::DIRECTORY { "755" } else { "644" }
            }.to_string();

            records.push(LsRecord {
                extension: ls::get_extension(&name),
                file_type: ls_file_type,
                size: metadata.len(),
                name,
                timestamp,
                octal_permissions: Some(octal_permissions),
            });
        }
    }
    Ok(records)
}

fn warn_unsupported_flags(flags: &[String]) {
    let mut unsupported = Vec::new();
    for f in flags {
        if f.starts_with("--") {
            if f != "--all" && f != "--full-time" && f != "--format=long" && f != "--format=verbose" {
                unsupported.push(f.clone());
            }
        } else if f.starts_with('-') && f != "-" {
            let bad_chars: String = f.chars().skip(1).filter(|c| *c != 'a' && *c != 'l' && *c != 't' && *c != 'r' && *c != 'h').collect();
            if !bad_chars.is_empty() {
                unsupported.push(f.clone());
            }
        }
    }

    if !unsupported.is_empty() {
        eprintln!(
            "{}",
            format!(
                "rtk ls: native Windows path ignores flags: {:?}",
                unsupported
            )
            .bold()
            .yellow()
        );
    }
}

pub fn run_native(paths: Vec<String>, options: FormatOptions, flags: Vec<String>) -> Result<(i32, String, String)> {
    warn_unsupported_flags(&flags);

    let mut records = fetch_entries(&paths, options.show_all)?;

    let sort_fn = if options.sort_by_time {
        |a: &LsRecord, b: &LsRecord| b.timestamp.unwrap_or(0).cmp(&a.timestamp.unwrap_or(0))
    } else {
        |a: &LsRecord, b: &LsRecord| a.name.cmp(&b.name)
    };

    records.sort_by(sort_fn);

    if options.reverse {
        records.reverse();
    }

    let raw_estimate = generate_mock_raw_output(&records);

    let (entries, summary) = super::ls_format::synthesize_output(records, &options);
    let is_tty = std::io::stdout().is_terminal();
    let output = if is_tty {
        format!("{}{}", entries, summary)
    } else {
        entries
    };

    Ok((0, output, raw_estimate))
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs::{self, File};
    use crate::cmds::system::ls_format::synthesize_output;

    #[test]
    fn test_fetch_entries_basic() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        fs::create_dir(dir_path.join("src")).unwrap();
        File::create(dir_path.join("Cargo.toml")).unwrap();
        File::create(dir_path.join("README.md")).unwrap();

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], false).unwrap();
        let options = FormatOptions::default();
        let (entries, _summary) = synthesize_output(records, &options);

        assert!(entries.contains("src/"));
        assert!(entries.contains("Cargo.toml"));
        assert!(entries.contains("README.md"));
        assert!(!entries.contains("total"));
    }

    #[test]
    fn test_fetch_entries_filters_noise() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        fs::create_dir(dir_path.join("node_modules")).unwrap();
        fs::create_dir(dir_path.join(".git")).unwrap();
        fs::create_dir(dir_path.join("target")).unwrap();
        fs::create_dir(dir_path.join("src")).unwrap();
        File::create(dir_path.join("main.rs")).unwrap();

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], false).unwrap();
        let options = FormatOptions::default();
        let (entries, _summary) = synthesize_output(records, &options);

        assert!(!entries.contains("node_modules"));
        assert!(!entries.contains(".git"));
        assert!(!entries.contains("target"));
        assert!(entries.contains("src/"));
        assert!(entries.contains("main.rs"));
    }

    #[test]
    fn test_fetch_entries_show_all() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        fs::create_dir(dir_path.join(".git")).unwrap();
        fs::create_dir(dir_path.join("src")).unwrap();

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], true).unwrap();
        let options = FormatOptions { show_all: true, show_long: false, sort_by_time: false, reverse: false };
        let (entries, _summary) = synthesize_output(records, &options);

        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_fetch_entries_empty() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], false).unwrap();
        let options = FormatOptions::default();
        let (entries, summary) = synthesize_output(records, &options);

        assert_eq!(entries, "(empty)\n");
        assert!(summary.is_empty());
    }

    #[test]
    fn test_fetch_entries_symlinks() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let target_path = dir_path.join("target.txt");
        File::create(&target_path).unwrap();

        let link_path = dir_path.join("link.txt");
        
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_file(&target_path, &link_path);
        
        #[cfg(not(windows))]
        let symlink_result = std::os::unix::fs::symlink(&target_path, &link_path);

        if let Err(e) = symlink_result {
            eprintln!("Failed to create symlink: {:?}", e);
            return;
        }

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], false).unwrap();
        let options = FormatOptions::default();
        let (entries, _summary) = synthesize_output(records, &options);

        assert!(
            entries.contains("link.txt -> target.txt") || entries.contains(&format!("link.txt -> {}", target_path.to_string_lossy())),
            "Symlink output does not include target, got entries:\n{}", 
            entries
        );
    }

    #[test]
    fn test_fetch_entries_summary() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        fs::create_dir(dir_path.join("src")).unwrap();
        File::create(dir_path.join("main.rs")).unwrap();
        File::create(dir_path.join("lib.rs")).unwrap();
        File::create(dir_path.join("Cargo.toml")).unwrap();

        let records = fetch_entries(&[dir_path.to_string_lossy().into_owned()], false).unwrap();
        let options = FormatOptions::default();
        let (_entries, summary) = synthesize_output(records, &options);

        assert!(summary.contains("Summary: 3 files, 1 dirs"));
        assert!(summary.contains(".rs"));
        assert!(summary.contains(".toml"));
    }

    #[test]
    fn test_fetch_entries_single_file() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        let file_path = dir_path.join("single.txt");
        File::create(&file_path).unwrap();

        let records = fetch_entries(&[file_path.to_string_lossy().into_owned()], false).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "single.txt");
        assert_eq!(records[0].file_type, LsRecordType::FILE);
    }

    #[test]
    fn test_run_native_sorting() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        File::create(dir_path.join("c.txt")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1050));
        
        File::create(dir_path.join("a.txt")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1050));
        
        File::create(dir_path.join("b.txt")).unwrap();

        let path_str = dir_path.to_string_lossy().into_owned();

        // 1. Default (alphabetical: a.txt -> b.txt -> c.txt)
        let options_default = FormatOptions::default();
        let (_, output_default, _) = run_native(vec![path_str.clone()], options_default, vec![]).unwrap();
        let pos_a = output_default.find("a.txt").unwrap();
        let pos_b = output_default.find("b.txt").unwrap();
        let pos_c = output_default.find("c.txt").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c, "Default sort should be alphabetical");

        // 2. Reverse alphabetical (-r: c.txt -> b.txt -> a.txt)
        let options_rev = FormatOptions { show_all: false, show_long: false, sort_by_time: false, reverse: true };
        let (_, output_rev, _) = run_native(vec![path_str.clone()], options_rev, vec!["-r".to_string()]).unwrap();
        let pos_a_r = output_rev.find("a.txt").unwrap();
        let pos_b_r = output_rev.find("b.txt").unwrap();
        let pos_c_r = output_rev.find("c.txt").unwrap();
        assert!(pos_c_r < pos_b_r && pos_b_r < pos_a_r, "Reverse sort (-r) should be reverse alphabetical");

        // 3. Time sort (-t: newest first -> b.txt -> a.txt -> c.txt)
        let options_time = FormatOptions { show_all: false, show_long: false, sort_by_time: true, reverse: false };
        let (_, output_time, _) = run_native(vec![path_str.clone()], options_time, vec!["-t".to_string()]).unwrap();
        let pos_a_t = output_time.find("a.txt").unwrap();
        let pos_b_t = output_time.find("b.txt").unwrap();
        let pos_c_t = output_time.find("c.txt").unwrap();
        assert!(pos_b_t < pos_a_t && pos_a_t < pos_c_t, "Time sort (-t) should be newest first");

        // 4. Reverse time sort (-rt: oldest first -> c.txt -> a.txt -> b.txt)
        let options_rev_time = FormatOptions { show_all: false, show_long: false, sort_by_time: true, reverse: true };
        let (_, output_rev_time, _) = run_native(vec![path_str.clone()], options_rev_time, vec!["-rt".to_string()]).unwrap();
        let pos_a_rt = output_rev_time.find("a.txt").unwrap();
        let pos_b_rt = output_rev_time.find("b.txt").unwrap();
        let pos_c_rt = output_rev_time.find("c.txt").unwrap();
        assert!(pos_c_rt < pos_a_rt && pos_a_rt < pos_b_rt, "Reverse time sort (-rt) should be oldest first");
    }

    #[test]
    fn test_run_native_total_combo() {
        let dir = tempdir().unwrap();
        let dir_path = dir.path();

        fs::create_dir(dir_path.join("folder_b")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        let file_path = dir_path.join("file_a.txt");
        File::create(&file_path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));

        let link_path = dir_path.join("symlink_c");
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_file(&file_path, &link_path);
        #[cfg(not(windows))]
        let symlink_result = std::os::unix::fs::symlink(&file_path, &link_path);

        if let Err(e) = symlink_result {
            eprintln!("Skipping symlink creation in combo test due to privileges: {:?}", e);
        }

        let path_str = dir_path.to_string_lossy().into_owned();

        let options = FormatOptions { show_all: true, show_long: false, sort_by_time: true, reverse: true };
        let (exit_code, output, estimate) = run_native(
            vec![path_str.clone()], 
            options.clone(), 
            vec!["-rt".to_string()]
        ).unwrap();

        assert_eq!(exit_code, 0);
        assert!(!estimate.is_empty(), "Token estimate string should be generated");
        
        assert!(output.contains("folder_b/"));
        assert!(output.contains("file_a.txt"));
        
        if dir_path.join("symlink_c").exists() {
            assert!(output.contains("symlink_c"));
        }

        let records = fetch_entries(&[path_str], true).unwrap();
        let (_entries, summary) = synthesize_output(records, &options);
        assert!(summary.contains("Summary: "));
    }
}



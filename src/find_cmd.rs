use anyhow::Result;
use std::collections::HashMap;
use std::process::Command;
use crate::tracking;

pub fn run(pattern: &str, path: &str, max_results: usize, verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("find: {} in {}", pattern, path);
    }

    let output = Command::new("fd")
        .args([pattern, path, "--type", "f"])
        .output()
        .or_else(|_| {
            Command::new("find")
                .args([path, "-name", pattern, "-type", "f"])
                .output()
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files: Vec<&str> = stdout.lines().collect();

    let raw_output = stdout.to_string();

    if files.is_empty() {
        let msg = format!("0 for '{}'", pattern);
        println!("{}", msg);
        tracking::track(&format!("find {} -name '{}'", path, pattern), "rtk find", &raw_output, &msg);
        return Ok(());
    }

    let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();

    for file in &files {
        let parts: Vec<&str> = file.rsplitn(2, '/').collect();
        let (filename, dir) = if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (parts[0].to_string(), ".".to_string())
        };
        by_dir.entry(dir).or_default().push(filename);
    }

    let mut dirs: Vec<_> = by_dir.keys().collect();
    dirs.sort();
    let dirs_count = dirs.len();

    println!("📁 {}F {}D:", files.len(), dirs_count);
    println!();

    let mut shown = 0;
    for dir in dirs {
        if shown >= max_results {
            println!("+{}", files.len() - shown);
            break;
        }

        let files_in_dir = &by_dir[dir];
        let dir_display = if dir.len() > 50 {
            format!("...{}", &dir[dir.len()-47..])
        } else {
            dir.clone()
        };

        println!("{}/ {}", dir_display, files_in_dir.join(" "));
        shown += files_in_dir.len();
    }

    let mut by_ext: HashMap<String, usize> = HashMap::new();
    for file in &files {
        let ext = file.rsplit('.').next().unwrap_or("none");
        *by_ext.entry(ext.to_string()).or_default() += 1;
    }

    let mut ext_line = String::new();
    if by_ext.len() > 1 {
        println!();
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by(|a, b| b.1.cmp(a.1));
        let ext_str: Vec<String> = exts.iter().take(5).map(|(e, c)| format!(".{}({})", e, c)).collect();
        ext_line = format!("ext: {}", ext_str.join(" "));
        println!("{}", ext_line);
    }

    let rtk_output = format!("{}F {}D + {}", files.len(), dirs_count, ext_line);
    tracking::track(&format!("find {} -name '{}'", path, pattern), "rtk find", &raw_output, &rtk_output);

    Ok(())
}

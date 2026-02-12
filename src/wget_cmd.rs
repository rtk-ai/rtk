use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Compact wget - strips progress bars, shows only result
pub fn run(url: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {}", url);
    }

    // Run wget normally but capture output to parse it
    let mut cmd_args: Vec<&str> = vec![];

    // Add user args
    for arg in args {
        cmd_args.push(arg);
    }
    cmd_args.push(url);

    let output = Command::new("wget")
        .args(&cmd_args)
        .output()
        .context("Failed to run wget")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let raw_output = format!("{}\n{}", stderr, stdout);

    if output.status.success() {
        let filename = extract_filename_from_output(&stderr, url, args);
        let size = get_file_size(&filename);
        let msg = format!(
            "⬇️ {} ok | {} | {}",
            compact_url(url),
            filename,
            format_size(size)
        );
        println!("{}", msg);
        timer.track(&format!("wget {}", url), "rtk wget", &raw_output, &msg);
    } else {
        let error = parse_error(&stderr, &stdout);
        let msg = format!("⬇️ {} FAILED: {}", compact_url(url), error);
        println!("{}", msg);
        timer.track(&format!("wget {}", url), "rtk wget", &raw_output, &msg);
    }

    Ok(())
}

/// Run wget and output to stdout (for piping)
pub fn run_stdout(url: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("wget: {} -> stdout", url);
    }

    let mut cmd_args = vec!["-q", "-O", "-"];
    for arg in args {
        cmd_args.push(arg);
    }
    cmd_args.push(url);

    let output = Command::new("wget")
        .args(&cmd_args)
        .output()
        .context("Failed to run wget")?;

    if output.status.success() {
        let content = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let raw_output = content.to_string();

        let mut rtk_output = String::new();
        if total > 20 {
            rtk_output.push_str(&format!(
                "⬇️ {} ok | {} lines | {}\n",
                compact_url(url),
                total,
                format_size(output.stdout.len() as u64)
            ));
            rtk_output.push_str("--- first 10 lines ---\n");
            for line in lines.iter().take(10) {
                rtk_output.push_str(&format!("{}\n", truncate_line(line, 100)));
            }
            rtk_output.push_str(&format!("... +{} more lines", total - 10));
        } else {
            rtk_output.push_str(&format!("⬇️ {} ok | {} lines\n", compact_url(url), total));
            for line in &lines {
                rtk_output.push_str(&format!("{}\n", line));
            }
        }
        print!("{}", rtk_output);
        timer.track(
            &format!("wget -O - {}", url),
            "rtk wget -o",
            &raw_output,
            &rtk_output,
        );
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = parse_error(&stderr, "");
        let msg = format!("⬇️ {} FAILED: {}", compact_url(url), error);
        println!("{}", msg);
        timer.track(&format!("wget -O - {}", url), "rtk wget -o", &stderr, &msg);
    }

    Ok(())
}

fn extract_filename_from_output(stderr: &str, url: &str, args: &[String]) -> String {
    // Check for -O argument first
    for (i, arg) in args.iter().enumerate() {
        if arg == "-O" || arg == "--output-document" {
            if let Some(name) = args.get(i + 1) {
                return name.clone();
            }
        }
        if let Some(name) = arg.strip_prefix("-O") {
            return name.to_string();
        }
    }

    // Parse wget output for "Sauvegarde en" or "Saving to"
    for line in stderr.lines() {
        // French: Sauvegarde en : « filename »
        if line.contains("Sauvegarde en") || line.contains("Saving to") {
            // Use char-based parsing to handle Unicode properly
            let chars: Vec<char> = line.chars().collect();
            let mut start_idx = None;
            let mut end_idx = None;

            for (i, c) in chars.iter().enumerate() {
                if *c == '«' || (*c == '\'' && start_idx.is_none()) {
                    start_idx = Some(i);
                }
                if *c == '»' || (*c == '\'' && start_idx.is_some()) {
                    end_idx = Some(i);
                }
            }

            if let (Some(s), Some(e)) = (start_idx, end_idx) {
                if e > s + 1 {
                    let filename: String = chars[s + 1..e].iter().collect();
                    return filename.trim().to_string();
                }
            }
        }
    }

    // Fallback: extract from URL
    let path = url.rsplit("://").next().unwrap_or(url);
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or("index.html")
        .split('?')
        .next()
        .unwrap_or("index.html");

    if filename.is_empty() || !filename.contains('.') {
        "index.html".to_string()
    } else {
        filename.to_string()
    }
}

fn get_file_size(filename: &str) -> u64 {
    std::fs::metadata(filename).map(|m| m.len()).unwrap_or(0)
}

fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "?".to_string();
    }
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn compact_url(url: &str) -> String {
    // Remove protocol
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);

    // Truncate if too long
    if without_proto.len() <= 50 {
        without_proto.to_string()
    } else {
        let prefix: String = without_proto.chars().take(25).collect();
        let suffix: String = without_proto.chars().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect();
        format!("{}...{}", prefix, suffix)
    }
}

fn parse_error(stderr: &str, stdout: &str) -> String {
    // Common wget error patterns
    let combined = format!("{}\n{}", stderr, stdout);

    if combined.contains("404") {
        return "404 Not Found".to_string();
    }
    if combined.contains("403") {
        return "403 Forbidden".to_string();
    }
    if combined.contains("401") {
        return "401 Unauthorized".to_string();
    }
    if combined.contains("500") {
        return "500 Server Error".to_string();
    }
    if combined.contains("Connection refused") {
        return "Connection refused".to_string();
    }
    if combined.contains("unable to resolve") || combined.contains("Name or service not known") {
        return "DNS lookup failed".to_string();
    }
    if combined.contains("timed out") {
        return "Connection timed out".to_string();
    }
    if combined.contains("SSL") || combined.contains("certificate") {
        return "SSL/TLS error".to_string();
    }

    // Return first meaningful line
    for line in stderr.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("--") {
            if trimmed.len() > 60 {
                let t: String = trimmed.chars().take(60).collect();
                return format!("{}...", t);
            }
            return trimmed.to_string();
        }
    }

    "Unknown error".to_string()
}

fn truncate_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        line.to_string()
    } else {
        let t: String = line.chars().take(max.saturating_sub(3)).collect();
        format!("{}...", t)
    }
}

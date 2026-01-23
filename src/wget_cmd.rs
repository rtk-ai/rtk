use anyhow::{Context, Result};
use std::process::Command;

/// Compact wget - strips progress bars, shows only result
pub fn run(url: &str, args: &[String], verbose: u8) -> Result<()> {
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

    if output.status.success() {
        // Extract filename from wget output or -O argument
        let filename = extract_filename_from_output(&stderr, url, args);

        // Get file size if exists
        let size = get_file_size(&filename);

        println!("⬇️ {} ok | {} | {}",
            compact_url(url),
            filename,
            format_size(size)
        );
    } else {
        // Parse error from stderr
        let error = parse_error(&stderr, &stdout);
        println!("⬇️ {} FAILED: {}", compact_url(url), error);
    }

    Ok(())
}

/// Run wget and output to stdout (for piping)
pub fn run_stdout(url: &str, args: &[String], verbose: u8) -> Result<()> {
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

        // Show summary instead of full content
        if total > 20 {
            println!("⬇️ {} ok | {} lines | {}",
                compact_url(url),
                total,
                format_size(output.stdout.len() as u64)
            );
            println!("--- first 10 lines ---");
            for line in lines.iter().take(10) {
                println!("{}", truncate_line(line, 100));
            }
            println!("... +{} more lines", total - 10);
        } else {
            println!("⬇️ {} ok | {} lines", compact_url(url), total);
            for line in &lines {
                println!("{}", line);
            }
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let error = parse_error(&stderr, "");
        println!("⬇️ {} FAILED: {}", compact_url(url), error);
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
        if arg.starts_with("-O") {
            return arg[2..].to_string();
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
    let filename = path.rsplit('/')
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
    std::fs::metadata(filename)
        .map(|m| m.len())
        .unwrap_or(0)
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
        format!("{}...{}", &without_proto[..25], &without_proto[without_proto.len()-20..])
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
                return format!("{}...", &trimmed[..60]);
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
        format!("{}...", &line[..max-3])
    }
}

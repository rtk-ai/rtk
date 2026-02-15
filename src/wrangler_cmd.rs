use crate::tracking;
use crate::utils::strip_ansi;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::process::Command;

/// Wrangler deploy output parsed into compact form
#[derive(Debug)]
struct DeployResult {
    worker_name: Option<String>,
    upload_size: Option<String>,
    gzip_size: Option<String>,
    deploy_time: Option<String>,
    urls: Vec<String>,
    version_id: Option<String>,
    warnings: Vec<String>,
    schedules: Vec<String>,
    errors: Vec<String>,
}

/// Wrangler pages deploy output
#[derive(Debug)]
struct PagesDeployResult {
    files_uploaded: Option<usize>,
    upload_time: Option<String>,
    url: Option<String>,
    errors: Vec<String>,
}

// ─── Deploy ────────────────────────────────────────────────────────

pub fn run_deploy(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("wrangler");
    cmd.arg("deploy");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: wrangler deploy {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run wrangler deploy (is wrangler installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_deploy_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("wrangler deploy {}", args.join(" ")),
        &format!("rtk wrangler deploy {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

fn parse_deploy_output(output: &str) -> DeployResult {
    let clean = strip_ansi(output);
    let mut result = DeployResult {
        worker_name: None,
        upload_size: None,
        gzip_size: None,
        deploy_time: None,
        urls: Vec::new(),
        version_id: None,
        warnings: Vec::new(),
        schedules: Vec::new(),
        errors: Vec::new(),
    };

    for line in clean.lines() {
        let trimmed = line.trim();

        // Extract worker name and deploy time: "Uploaded my-worker (1.23 sec)"
        if trimmed.starts_with("Uploaded ") {
            if let Some(rest) = trimmed.strip_prefix("Uploaded ") {
                if let Some(paren_idx) = rest.find('(') {
                    result.worker_name = Some(rest[..paren_idx].trim().to_string());
                    let time_part = &rest[paren_idx + 1..];
                    if let Some(end_idx) = time_part.find(')') {
                        result.deploy_time = Some(time_part[..end_idx].trim().to_string());
                    }
                } else {
                    result.worker_name = Some(rest.trim().to_string());
                }
            }
        }

        // Extract upload size: "Total Upload: 45.67 KiB / gzip: 12.34 KiB"
        if trimmed.starts_with("Total Upload:") {
            if let Some(rest) = trimmed.strip_prefix("Total Upload:") {
                let rest = rest.trim();
                if let Some(slash_idx) = rest.find('/') {
                    result.upload_size = Some(rest[..slash_idx].trim().to_string());
                    let gzip_part = &rest[slash_idx + 1..];
                    if let Some(gzip_rest) = gzip_part.trim().strip_prefix("gzip:") {
                        result.gzip_size = Some(gzip_rest.trim().to_string());
                    }
                } else {
                    result.upload_size = Some(rest.to_string());
                }
            }
        }

        // Extract URLs (lines starting with https://)
        if trimmed.starts_with("https://") {
            result.urls.push(trimmed.to_string());
        }

        // Extract version ID: "Current Version ID: abc123"
        if trimmed.starts_with("Current Version ID:") {
            if let Some(rest) = trimmed.strip_prefix("Current Version ID:") {
                result.version_id = Some(rest.trim().to_string());
            }
        }

        // Extract schedule triggers: "schedule: */5 * * * *"
        if trimmed.starts_with("schedule:") {
            result.schedules.push(trimmed.to_string());
        }

        // Extract warnings
        if trimmed.contains("[WARNING]") || trimmed.contains("[WARN]") {
            result.warnings.push(trimmed.to_string());
        }

        // Extract binding types from warning context
        // Lines like "  - KV Namespaces: MY_KV" or "  - D1 Databases: MY_DB"
        if trimmed.starts_with("- ") && trimmed.contains(':') {
            let binding = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            if let Some(colon_idx) = binding.find(':') {
                let binding_type = &binding[..colon_idx];
                // Only capture known Cloudflare binding types
                let known_bindings = [
                    "KV Namespaces",
                    "D1 Databases",
                    "R2 Buckets",
                    "Durable Objects",
                    "Service Bindings",
                    "Queue Producers",
                    "Queue Consumers",
                    "Analytics Engine",
                    "Vectorize Indexes",
                    "Hyperdrive Configs",
                    "AI",
                    "Browser",
                    "mTLS Certificates",
                    "Vars",
                ];
                if known_bindings
                    .iter()
                    .any(|b| binding_type.trim() == *b)
                {
                    // These get summarized in the warning line
                }
            }
        }

        // Extract errors
        if trimmed.contains("[ERROR]") || trimmed.starts_with("✘") || trimmed.starts_with("Error:") {
            result.errors.push(trimmed.to_string());
        }
    }

    result
}

fn extract_binding_types(output: &str) -> Vec<String> {
    let clean = strip_ansi(output);
    let mut bindings = Vec::new();
    let known = [
        ("KV Namespaces", "KV"),
        ("D1 Databases", "D1"),
        ("R2 Buckets", "R2"),
        ("Durable Objects", "DO"),
        ("Service Bindings", "Service"),
        ("Queue Producers", "Queue"),
        ("Queue Consumers", "Queue"),
        ("Analytics Engine", "AE"),
        ("Vectorize Indexes", "Vectorize"),
        ("Hyperdrive Configs", "Hyperdrive"),
        ("AI", "AI"),
        ("Browser", "Browser"),
        ("Vars", "Vars"),
    ];

    let mut seen = HashSet::new();
    for line in clean.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- ") {
            for (full, short) in &known {
                if trimmed.contains(full) && seen.insert(*short) {
                    bindings.push(short.to_string());
                }
            }
        }
    }

    bindings
}

fn filter_deploy_output(output: &str) -> String {
    let result = parse_deploy_output(output);

    // If there are errors, show them prominently
    if !result.errors.is_empty() {
        let mut lines = vec!["✘ wrangler deploy failed".to_string()];
        for err in &result.errors {
            lines.push(format!("  {}", err));
        }
        return lines.join("\n");
    }

    let mut lines = Vec::new();

    // Main status line
    let name = result.worker_name.as_deref().unwrap_or("worker");
    let time = result
        .deploy_time
        .as_ref()
        .map(|t| format!(" ({})", t))
        .unwrap_or_default();
    let size = match (&result.upload_size, &result.gzip_size) {
        (Some(upload), Some(gzip)) => format!(" {}/{} gzip", upload, gzip),
        (Some(upload), None) => format!(" {}", upload),
        _ => String::new(),
    };
    lines.push(format!("✓ {} deployed{}{}", name, time, size));

    // URLs
    for url in &result.urls {
        lines.push(format!("  {}", url));
    }

    // Schedules
    for schedule in &result.schedules {
        lines.push(format!("  {}", schedule));
    }

    // Version
    if let Some(ref version) = result.version_id {
        lines.push(format!("  version: {}", version));
    }

    // Warnings summary
    let binding_types = extract_binding_types(output);
    let warning_count = result.warnings.len();
    if warning_count > 0 {
        let bindings_str = if binding_types.is_empty() {
            String::new()
        } else {
            format!(" (bindings: {})", binding_types.join(", "))
        };
        lines.push(format!(
            "  ⚠ {} warning{}{}",
            warning_count,
            if warning_count == 1 { "" } else { "s" },
            bindings_str
        ));
    }

    if result.worker_name.is_none()
        && result.urls.is_empty()
        && result.version_id.is_none()
        && result.errors.is_empty()
    {
        return "ok ✓".to_string();
    }

    lines.join("\n")
}

// ─── Pages Deploy ──────────────────────────────────────────────────

pub fn run_pages(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("wrangler");
    cmd.arg("pages");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: wrangler pages {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run wrangler pages (is wrangler installed?)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_pages_output(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("wrangler pages {}", args.join(" ")),
        &format!("rtk wrangler pages {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

fn parse_pages_output(output: &str) -> PagesDeployResult {
    let clean = strip_ansi(output);
    let mut result = PagesDeployResult {
        files_uploaded: None,
        upload_time: None,
        url: None,
        errors: Vec::new(),
    };

    for line in clean.lines() {
        let trimmed = line.trim();

        // "✨ Success! Uploaded 24 files (2.34 sec)"
        if trimmed.contains("Uploaded") && trimmed.contains("files") {
            // Extract file count
            for word in trimmed.split_whitespace() {
                if let Ok(n) = word.parse::<usize>() {
                    result.files_uploaded = Some(n);
                    break;
                }
            }
            // Extract time
            if let Some(paren_idx) = trimmed.find('(') {
                let time_part = &trimmed[paren_idx + 1..];
                if let Some(end_idx) = time_part.find(')') {
                    result.upload_time = Some(time_part[..end_idx].trim().to_string());
                }
            }
        }

        // "✨ Deployment complete! Take a peek over at https://..."
        if trimmed.contains("https://") {
            for word in trimmed.split_whitespace() {
                if word.starts_with("https://") {
                    result.url = Some(word.to_string());
                    break;
                }
            }
        }

        // Errors
        if trimmed.contains("[ERROR]") || trimmed.starts_with("✘") || trimmed.starts_with("Error:") {
            result.errors.push(trimmed.to_string());
        }
    }

    result
}

fn filter_pages_output(output: &str) -> String {
    let result = parse_pages_output(output);

    if !result.errors.is_empty() {
        let mut lines = vec!["✘ wrangler pages deploy failed".to_string()];
        for err in &result.errors {
            lines.push(format!("  {}", err));
        }
        return lines.join("\n");
    }

    let mut lines = Vec::new();

    let files = result
        .files_uploaded
        .map(|n| format!("{} files", n))
        .unwrap_or_else(|| "files".to_string());
    let time = result
        .upload_time
        .as_ref()
        .map(|t| format!(" ({})", t))
        .unwrap_or_default();
    lines.push(format!("✓ {} uploaded{}", files, time));

    if let Some(ref url) = result.url {
        lines.push(format!("  {}", url));
    }

    if lines.is_empty() {
        "ok ✓".to_string()
    } else {
        lines.join("\n")
    }
}

// ─── Dev ───────────────────────────────────────────────────────────

pub fn run_dev(args: &[String], verbose: u8) -> Result<()> {
    // Dev is interactive - we passthrough but filter the initial noise
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("wrangler");
    cmd.arg("dev");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: wrangler dev {}", args.join(" "));
    }

    // For dev, we inherit stdio since it's interactive
    let status = cmd
        .status()
        .context("Failed to run wrangler dev (is wrangler installed?)")?;

    timer.track_passthrough(
        &format!("wrangler dev {}", args.join(" ")),
        &format!("rtk wrangler dev {}", args.join(" ")),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

// ─── Passthrough ───────────────────────────────────────────────────

pub fn run_passthrough(args: &[std::ffi::OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("wrangler");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: wrangler {} (passthrough)",
            args.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        );
    }

    let status = cmd.status().context("Failed to run wrangler")?;

    let args_str = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    timer.track_passthrough(
        &format!("wrangler {}", args_str),
        &format!("rtk wrangler {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Deploy tests ───────────────────────────────────────────

    #[test]
    fn test_deploy_basic() {
        let output = r#"
 ⛅️ wrangler 4.14.4 (update available 4.15.0)
-------------------------------------------------------

Total Upload: 45.67 KiB / gzip: 12.34 KiB
Worker Startup Time: 2 ms
Uploaded my-worker (1.23 sec)
Deployed my-worker triggers (0.45 sec)
  https://my-worker.username.workers.dev
Current Version ID: abc123def456
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("✓ my-worker deployed"));
        assert!(result.contains("1.23 sec"));
        assert!(result.contains("45.67 KiB"));
        assert!(result.contains("12.34 KiB"));
        assert!(result.contains("https://my-worker.username.workers.dev"));
        assert!(result.contains("version: abc123def456"));
        // Should NOT contain banner
        assert!(!result.contains("⛅️"));
        assert!(!result.contains("-------"));
        assert!(!result.contains("Worker Startup Time"));
    }

    #[test]
    fn test_deploy_with_warnings_and_bindings() {
        let output = r#"
 ⛅️ wrangler 4.14.4
-------------------------------------------------------

▲ [WARNING] Your worker has access to the following bindings

  - KV Namespaces: MY_KV
  - D1 Databases: MY_DB

Total Upload: 123.45 KiB / gzip: 34.56 KiB
Worker Startup Time: 5 ms
Uploaded api-worker (2.34 sec)
Deployed api-worker triggers (0.67 sec)
  https://api-worker.username.workers.dev
  https://api.example.com
Current Version ID: xyz789abc012
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("✓ api-worker deployed"));
        assert!(result.contains("https://api-worker.username.workers.dev"));
        assert!(result.contains("https://api.example.com"));
        assert!(result.contains("⚠ 1 warning"));
        assert!(result.contains("KV"));
        assert!(result.contains("D1"));
    }

    #[test]
    fn test_deploy_with_schedule() {
        let output = r#"
Total Upload: 10.00 KiB / gzip: 3.00 KiB
Uploaded cron-worker (0.50 sec)
Deployed cron-worker triggers (0.20 sec)
  https://cron-worker.username.workers.dev
  schedule: */5 * * * *
Current Version ID: schedule123
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("✓ cron-worker deployed"));
        assert!(result.contains("schedule: */5 * * * *"));
    }

    #[test]
    fn test_deploy_multiple_warnings() {
        let output = r#"
▲ [WARNING] First warning

▲ [WARNING] Second warning

  - KV Namespaces: CACHE
  - R2 Buckets: STORAGE

Total Upload: 50.00 KiB / gzip: 15.00 KiB
Uploaded multi-worker (1.00 sec)
  https://multi-worker.workers.dev
Current Version ID: multi123
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("⚠ 2 warnings"));
        assert!(result.contains("KV"));
        assert!(result.contains("R2"));
    }

    #[test]
    fn test_deploy_error() {
        let output = r#"
 ⛅️ wrangler 4.14.4
-------------------------------------------------------

✘ [ERROR] Could not find wrangler.toml

  No configuration file found. Create a wrangler.toml in your project root.
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("✘ wrangler deploy failed"));
        assert!(result.contains("Could not find wrangler.toml"));
    }

    #[test]
    fn test_deploy_only_size_no_gzip() {
        let output = r#"
Total Upload: 5.00 KiB
Uploaded simple-worker (0.30 sec)
  https://simple.workers.dev
Current Version ID: simple123
"#;
        let result = filter_deploy_output(output);
        assert!(result.contains("5.00 KiB"));
        assert!(!result.contains("gzip"));
    }

    #[test]
    fn test_deploy_empty_output() {
        let result = filter_deploy_output("");
        assert_eq!(result, "ok ✓");
    }

    #[test]
    fn test_deploy_ansi_codes_stripped() {
        let output = "\x1b[32mUploaded\x1b[0m \x1b[1mmy-worker\x1b[0m (1.00 sec)\n  https://my-worker.workers.dev\nCurrent Version ID: ansi123\n";
        let result = filter_deploy_output(output);
        assert!(result.contains("my-worker deployed"));
        assert!(!result.contains("\x1b"));
    }

    // ── Pages deploy tests ─────────────────────────────────────

    #[test]
    fn test_pages_deploy_basic() {
        let output = r#"
🌍 Uploading... (24/24)

✨ Success! Uploaded 24 files (2.34 sec)

✨ Deployment complete! Take a peek over at https://abc123.my-pages.pages.dev
"#;
        let result = filter_pages_output(output);
        assert!(result.contains("✓ 24 files uploaded"));
        assert!(result.contains("2.34 sec"));
        assert!(result.contains("https://abc123.my-pages.pages.dev"));
        // Should NOT contain progress
        assert!(!result.contains("Uploading..."));
        assert!(!result.contains("🌍"));
    }

    #[test]
    fn test_pages_deploy_error() {
        let output = r#"
✘ [ERROR] Failed to publish

  No project found with name "nonexistent"
"#;
        let result = filter_pages_output(output);
        assert!(result.contains("✘ wrangler pages deploy failed"));
        assert!(result.contains("Failed to publish"));
    }

    #[test]
    fn test_pages_deploy_no_count() {
        let output = r#"
✨ Deployment complete! Take a peek over at https://preview.my-pages.pages.dev
"#;
        let result = filter_pages_output(output);
        assert!(result.contains("✓ files uploaded"));
        assert!(result.contains("https://preview.my-pages.pages.dev"));
    }

    // ── Parse helpers ──────────────────────────────────────────

    #[test]
    fn test_parse_deploy_output_fields() {
        let output = r#"
Total Upload: 100.00 KiB / gzip: 30.00 KiB
Uploaded test-worker (3.50 sec)
  https://test.workers.dev
  https://custom.example.com
  schedule: 0 * * * *
Current Version ID: test789
"#;
        let result = parse_deploy_output(output);
        assert_eq!(result.worker_name, Some("test-worker".to_string()));
        assert_eq!(result.upload_size, Some("100.00 KiB".to_string()));
        assert_eq!(result.gzip_size, Some("30.00 KiB".to_string()));
        assert_eq!(result.deploy_time, Some("3.50 sec".to_string()));
        assert_eq!(result.urls.len(), 2);
        assert_eq!(result.version_id, Some("test789".to_string()));
        assert_eq!(result.schedules.len(), 1);
    }

    #[test]
    fn test_extract_binding_types() {
        let output = r#"
  - KV Namespaces: CACHE
  - D1 Databases: MY_DB
  - R2 Buckets: STORAGE
  - Durable Objects: COUNTER
  - AI: AI_BINDING
  - Vars: API_KEY
"#;
        let bindings = extract_binding_types(output);
        assert!(bindings.contains(&"KV".to_string()));
        assert!(bindings.contains(&"D1".to_string()));
        assert!(bindings.contains(&"R2".to_string()));
        assert!(bindings.contains(&"DO".to_string()));
        assert!(bindings.contains(&"AI".to_string()));
        assert!(bindings.contains(&"Vars".to_string()));
    }

    #[test]
    fn test_extract_binding_types_dedup() {
        let output = r#"
  - Queue Producers: QUEUE_1
  - Queue Consumers: QUEUE_1
"#;
        let bindings = extract_binding_types(output);
        // Queue should appear only once despite two entries
        assert_eq!(
            bindings.iter().filter(|b| *b == "Queue").count(),
            1,
            "Queue binding should be deduplicated"
        );
    }

    #[test]
    fn test_parse_pages_output_fields() {
        let output = r#"
✨ Success! Uploaded 42 files (5.67 sec)
✨ Deployment complete! Take a peek over at https://deploy.pages.dev
"#;
        let result = parse_pages_output(output);
        assert_eq!(result.files_uploaded, Some(42));
        assert_eq!(result.upload_time, Some("5.67 sec".to_string()));
        assert_eq!(
            result.url,
            Some("https://deploy.pages.dev".to_string())
        );
        assert!(result.errors.is_empty());
    }
}

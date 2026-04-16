//! Confluence filter module — `rtk acli confluence <object> <verb> [args...]`

use anyhow::Result;
use serde_json::Value;

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};

/// Max characters of body content to include (keeps LLM context focused).
const MAX_BODY_CHARS: usize = 3000;

/// Entry point: `args` = ["page", "view", "--id", "123"] etc.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let object = args.first().map(|s| s.as_str()).unwrap_or("");
    let verb = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest = if args.len() > 2 { &args[2..] } else { &[][..] };

    match (object, verb) {
        ("page", "view") => run_page_view(rest, verbose),
        _ => run_passthrough(args, verbose),
    }
}

/// Specialised runner for `page view` — injects `--body-format atlas_doc_format`
/// so page content is always returned and can be extracted.
fn run_page_view(extra_args: &[String], verbose: u8) -> Result<i32> {
    let sub_args: &[&str] = &["confluence", "page", "view"];
    let tee_slug = "acli_confluence_page_view";
    let timer = tracking::TimedExecution::start();
    let cmd_label = "acli confluence page view".to_string();
    let rtk_label = format!("rtk {}", cmd_label);

    if verbose > 0 {
        eprintln!(
            "rtk acli confluence: running acli {} {}",
            sub_args.join(" "),
            extra_args.join(" ")
        );
    }

    let mut cmd = resolved_command("acli");
    cmd.args(sub_args);
    cmd.args(extra_args);

    let has_json = extra_args.iter().any(|a| a == "--json");
    let has_body_format = extra_args
        .windows(2)
        .any(|w| w[0] == "--body-format" || w[0] == "--body-format=atlas_doc_format")
        || extra_args
            .iter()
            .any(|a| a.starts_with("--body-format="));

    if !has_json {
        cmd.arg("--json");
    }
    if !has_body_format {
        // atlas_doc_format gives a clean ADF JSON tree — easier to parse than storage XML
        cmd.arg("--body-format").arg("atlas_doc_format");
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if exit_code != 0 {
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprint!("{}", stderr);
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let filtered = filter_page_view(&stdout);
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, 0) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &filtered);
    Ok(0)
}

/// Passthrough for unhandled confluence subcommands.
fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("rtk acli confluence: passthrough for '{}'", args.join(" "));
    }
    let mut cmd_args: Vec<String> = vec!["confluence".to_string()];
    cmd_args.extend_from_slice(args);

    let output = resolved_command("acli")
        .args(&cmd_args)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli confluence: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    let raw = format!("{}\n{}", stdout, stderr);
    timer.track(
        &format!("acli confluence {}", args.join(" ")),
        &format!("rtk acli confluence {} (passthrough)", args.join(" ")),
        &raw,
        &raw,
    );

    Ok(exit_code)
}

// ---------------------------------------------------------------------------
// Filter functions
// ---------------------------------------------------------------------------

/// Filter `acli confluence page view --body-format atlas_doc_format --json` output.
///
/// Extracts: title, status, version, URL, and full page body content.
pub fn filter_page_view(raw: &str) -> String {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return raw.to_string(),
    };

    let title = v["title"].as_str().unwrap_or("").to_string();
    let status = v["status"].as_str().unwrap_or("").to_string();
    let version = v["version"]["number"]
        .as_u64()
        .map(|n| format!("v{}", n))
        .unwrap_or_default();
    let webui = v["_links"]["webui"].as_str().unwrap_or("").to_string();
    let base = v["_links"]["base"].as_str().unwrap_or("").to_string();

    let url = if webui.starts_with("http") {
        webui
    } else if !base.is_empty() && !webui.is_empty() {
        format!("{}{}", base, webui)
    } else {
        webui
    };

    // Extract body from atlas_doc_format (value is a JSON string inside the object)
    let body_text = extract_confluence_body(&v);

    let mut lines: Vec<String> = Vec::new();

    if !title.is_empty() {
        lines.push(title);
    }
    let v_str = if version.is_empty() {
        String::new()
    } else {
        format!("  {}", version)
    };
    if !status.is_empty() || !version.is_empty() {
        lines.push(format!("Status: {}{}", status, v_str));
    }
    if !url.is_empty() {
        lines.push(format!("URL: {}", url));
    }

    if !body_text.is_empty() {
        lines.push(String::new());
        lines.push("---".to_string());
        lines.push(truncate(&body_text, MAX_BODY_CHARS));
    }

    lines.join("\n")
}

/// Extract body text from the atlas_doc_format ADF tree embedded in the page JSON.
fn extract_confluence_body(page: &Value) -> String {
    // Body is at page["body"]["atlas_doc_format"]["value"] — a JSON *string*
    let adf_str = match page["body"]["atlas_doc_format"]["value"].as_str() {
        Some(s) => s,
        None => return String::new(),
    };

    let doc: Value = match serde_json::from_str(adf_str) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };

    // Reuse the same ADF walker from jira.rs
    crate::cmds::atlassian::jira::adf_to_text(&doc)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_page_view_snapshot() {
        let input =
            include_str!("../../../tests/fixtures/acli_confluence_page_view_raw.txt");
        let output = filter_page_view(input);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_page_view_savings() {
        let input =
            include_str!("../../../tests/fixtures/acli_confluence_page_view_raw.txt");
        let output = filter_page_view(input);
        let pct =
            100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            pct >= 60.0,
            "page view: expected ≥60% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_page_view_empty() {
        let _ = filter_page_view("");
    }

    #[test]
    fn test_page_view_not_json() {
        let out = filter_page_view("✗ Error: page not found");
        assert_eq!(out, "✗ Error: page not found");
    }

    #[test]
    fn test_page_view_extracts_title_and_meta() {
        let input =
            include_str!("../../../tests/fixtures/acli_confluence_page_view_raw.txt");
        let output = filter_page_view(input);
        assert!(
            output.contains("API Authentication Guide"),
            "should contain title"
        );
        assert!(output.contains("current"), "should contain status");
        assert!(output.contains("v3"), "should contain version");
        assert!(output.contains("URL:"), "should contain URL");
    }

    #[test]
    fn test_page_view_has_body_content() {
        let input =
            include_str!("../../../tests/fixtures/acli_confluence_page_view_raw.txt");
        let output = filter_page_view(input);
        assert!(
            output.contains("---"),
            "should have a separator before body"
        );
        // The page is about JWT authentication
        assert!(
            output.to_lowercase().contains("jwt")
                || output.to_lowercase().contains("auth"),
            "should contain page body content"
        );
    }
}

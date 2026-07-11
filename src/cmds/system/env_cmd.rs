//! Filters environment variables, compacting noise.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use anyhow::Result;
use std::env;
use std::fmt::Write;

/// Show filtered environment variables
pub fn run(filter: Option<&str>, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Environment variables:");
    }

    let mut vars: Vec<(String, String)> = env::vars().collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));

    // Interesting categories
    let mut path_vars = Vec::new();
    let mut lang_vars = Vec::new();
    let mut cloud_vars = Vec::new();
    let mut tool_vars = Vec::new();
    let mut other_vars = Vec::new();

    for (key, value) in &vars {
        // Apply filter if provided
        if let Some(f) = filter {
            if !key.to_lowercase().contains(&f.to_lowercase()) {
                continue;
            }
        }

        let display_value = if value.len() > 100 {
            let preview: String = value.chars().take(50).collect();
            format!("{}... ({} chars)", preview, value.chars().count())
        } else {
            value.clone()
        };

        let entry = (key.clone(), display_value);

        // Categorize
        if key.contains("PATH") {
            path_vars.push(entry);
        } else if is_lang_var(key) {
            lang_vars.push(entry);
        } else if is_cloud_var(key) {
            cloud_vars.push(entry);
        } else if is_tool_var(key) {
            tool_vars.push(entry);
        } else if filter.is_some() || is_interesting_var(key) {
            other_vars.push(entry);
        }
    }

    let mut body = String::new();
    if !path_vars.is_empty() {
        let _ = writeln!(body, "PATH Variables:");
        for (k, v) in &path_vars {
            if k == "PATH" {
                // Split PATH for readability
                let paths: Vec<&str> = v.split(':').collect();
                let _ = writeln!(body, "  PATH ({} entries):", paths.len());
                const MAX_PATH_ENTRIES: usize = CAP_WARNINGS;
                for p in paths.iter().take(MAX_PATH_ENTRIES) {
                    let _ = writeln!(body, "    {}", p);
                }
                if paths.len() > MAX_PATH_ENTRIES {
                    let _ = writeln!(body, "    ... +{} more", paths.len() - MAX_PATH_ENTRIES);
                }
            } else {
                let _ = writeln!(body, "  {}={}", k, v);
            }
        }
    }

    if !lang_vars.is_empty() {
        let _ = writeln!(body, "\nLanguage/Runtime:");
        for (k, v) in &lang_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !cloud_vars.is_empty() {
        let _ = writeln!(body, "\nCloud/Services:");
        for (k, v) in &cloud_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !tool_vars.is_empty() {
        let _ = writeln!(body, "\nTools:");
        for (k, v) in &tool_vars {
            let _ = writeln!(body, "  {}={}", k, v);
        }
    }

    if !other_vars.is_empty() {
        const MAX_OTHER_VARS: usize = CAP_LIST;
        let _ = writeln!(body, "\nOther:");
        for (k, v) in other_vars.iter().take(MAX_OTHER_VARS) {
            let _ = writeln!(body, "  {}={}", k, v);
        }
        if other_vars.len() > MAX_OTHER_VARS {
            let _ = writeln!(body, "  ... +{} more", other_vars.len() - MAX_OTHER_VARS);
        }
    }

    let total = vars.len();
    let shown = path_vars.len()
        + lang_vars.len()
        + cloud_vars.len()
        + tool_vars.len()
        + other_vars.len().min(20);
    if filter.is_none() {
        let _ = writeln!(body, "\nTotal: {} vars (showing {} relevant)", total, shown);
    }

    let raw: String = vars.iter().fold(String::new(), |mut output, (k, v)| {
        let _ = writeln!(output, "{}={}", k, v);
        output
    });
    let shown_body = never_worse(&raw, &body);
    print!("{}", shown_body);
    timer.track("env", "rtk env", &raw, shown_body);
    Ok(())
}

fn is_lang_var(key: &str) -> bool {
    let patterns = [
        "RUST", "CARGO", "PYTHON", "PIP", "NODE", "NPM", "YARN", "DENO", "BUN", "JAVA", "MAVEN",
        "GRADLE", "GO", "GOPATH", "GOROOT", "RUBY", "GEM", "PERL", "PHP", "DOTNET", "NUGET",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_cloud_var(key: &str) -> bool {
    let patterns = [
        "AWS",
        "AZURE",
        "GCP",
        "GOOGLE_CLOUD",
        "DOCKER",
        "KUBERNETES",
        "K8S",
        "HELM",
        "TERRAFORM",
        "VAULT",
        "CONSUL",
        "NOMAD",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_tool_var(key: &str) -> bool {
    let patterns = [
        "EDITOR",
        "VISUAL",
        "SHELL",
        "TERM",
        "GIT",
        "SSH",
        "GPG",
        "BREW",
        "HOMEBREW",
        "XDG",
        "CLAUDE",
        "ANTHROPIC",
    ];
    patterns.iter().any(|p| key.to_uppercase().contains(p))
}

fn is_interesting_var(key: &str) -> bool {
    let patterns = ["HOME", "USER", "LANG", "LC_", "TZ", "PWD", "OLDPWD"];
    patterns.iter().any(|p| key.to_uppercase().starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_lang_var_rust() {
        assert!(is_lang_var("RUST_LOG"));
        assert!(is_lang_var("CARGO_HOME"));
        assert!(is_lang_var("GOPATH"));
        assert!(is_lang_var("NODE_ENV"));
    }

    #[test]
    fn test_is_lang_var_negative() {
        assert!(!is_lang_var("HOME"));
        assert!(!is_lang_var("PATH"));
        assert!(!is_lang_var("USER"));
    }

    #[test]
    fn test_is_cloud_var() {
        assert!(is_cloud_var("AWS_ACCESS_KEY_ID"));
        assert!(is_cloud_var("AZURE_CLIENT_ID"));
        assert!(is_cloud_var("DOCKER_HOST"));
        assert!(is_cloud_var("KUBERNETES_SERVICE_HOST"));
    }

    #[test]
    fn test_is_cloud_var_negative() {
        assert!(!is_cloud_var("HOME"));
        assert!(!is_cloud_var("RUST_LOG"));
    }

    #[test]
    fn test_is_tool_var() {
        assert!(is_tool_var("EDITOR"));
        assert!(is_tool_var("GIT_AUTHOR_NAME"));
        assert!(is_tool_var("SSH_AUTH_SOCK"));
        assert!(is_tool_var("CLAUDE_API_KEY"));
    }

    #[test]
    fn test_is_interesting_var() {
        assert!(is_interesting_var("HOME"));
        assert!(is_interesting_var("USER"));
        assert!(is_interesting_var("LANG"));
        assert!(is_interesting_var("TZ"));
        assert!(is_interesting_var("PWD"));
    }

    #[test]
    fn test_is_interesting_var_negative() {
        assert!(!is_interesting_var("RANDOM_VAR"));
        assert!(!is_interesting_var("MY_CUSTOM_VAR"));
    }
}

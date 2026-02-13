use crate::tracking;
use anyhow::Result;
use std::collections::HashSet;
use std::env;

/// Show filtered environment variables (hide sensitive data)
pub fn run(filter: Option<&str>, show_all: bool, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Environment variables:");
    }

    let sensitive_patterns = get_sensitive_patterns();
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

        // Check if sensitive
        let is_sensitive = sensitive_patterns
            .iter()
            .any(|p| key.to_lowercase().contains(p));

        let display_value = if is_sensitive && !show_all {
            mask_value(value)
        } else if value.len() > 100 {
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

    // Print categorized
    if !path_vars.is_empty() {
        println!("📂 PATH Variables:");
        for (k, v) in &path_vars {
            if k == "PATH" {
                // Split PATH for readability
                let paths: Vec<&str> = v.split(':').collect();
                println!("  PATH ({} entries):", paths.len());
                for p in paths.iter().take(5) {
                    println!("    {}", p);
                }
                if paths.len() > 5 {
                    println!("    ... +{} more", paths.len() - 5);
                }
            } else {
                println!("  {}={}", k, v);
            }
        }
    }

    if !lang_vars.is_empty() {
        println!("\n🔧 Language/Runtime:");
        for (k, v) in &lang_vars {
            println!("  {}={}", k, v);
        }
    }

    if !cloud_vars.is_empty() {
        println!("\n☁️  Cloud/Services:");
        for (k, v) in &cloud_vars {
            println!("  {}={}", k, v);
        }
    }

    if !tool_vars.is_empty() {
        println!("\n🛠️  Tools:");
        for (k, v) in &tool_vars {
            println!("  {}={}", k, v);
        }
    }

    if !other_vars.is_empty() {
        println!("\n📋 Other:");
        for (k, v) in other_vars.iter().take(20) {
            println!("  {}={}", k, v);
        }
        if other_vars.len() > 20 {
            println!("  ... +{} more", other_vars.len() - 20);
        }
    }

    let total = vars.len();
    let shown = path_vars.len()
        + lang_vars.len()
        + cloud_vars.len()
        + tool_vars.len()
        + other_vars.len().min(20);
    if filter.is_none() {
        println!("\n📊 Total: {} vars (showing {} relevant)", total, shown);
    }

    let raw: String = vars.iter().map(|(k, v)| format!("{}={}\n", k, v)).collect();
    let rtk = format!("{} vars -> {} shown", total, shown);
    timer.track("env", "rtk env", &raw, &rtk);
    Ok(())
}

fn get_sensitive_patterns() -> HashSet<&'static str> {
    let mut set = HashSet::new();
    set.insert("key");
    set.insert("secret");
    set.insert("password");
    set.insert("token");
    set.insert("credential");
    set.insert("auth");
    set.insert("private");
    set.insert("api_key");
    set.insert("apikey");
    set.insert("access_key");
    set.insert("jwt");
    set
}

fn mask_value(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 4 {
        "****".to_string()
    } else {
        let prefix: String = chars[..2].iter().collect();
        let suffix: String = chars[chars.len() - 2..].iter().collect();
        format!("{}****{}", prefix, suffix)
    }
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

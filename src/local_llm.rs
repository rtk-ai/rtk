use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::filter::Language;

/// Heuristic-based code summarizer - no external model needed
pub fn run(file: &Path, _model: &str, _force_download: bool, verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("Analyzing: {}", file.display());
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read file: {}", file.display()))?;

    let lang = file
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    let summary = analyze_code(&content, &lang);

    println!("{}", summary.line1);
    println!("{}", summary.line2);

    Ok(())
}

struct CodeSummary {
    line1: String,
    line2: String,
}

fn analyze_code(content: &str, lang: &Language) -> CodeSummary {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Extract components
    let imports = extract_imports(content, lang);
    let functions = extract_functions(content, lang);
    let structs = extract_structs(content, lang);
    let traits = extract_traits(content, lang);

    // Detect patterns
    let patterns = detect_patterns(content, lang);

    // Build line 1: What it is
    let lang_name = lang_display_name(lang);
    let main_type = if !structs.is_empty() && !functions.is_empty() {
        format!("{} module", lang_name)
    } else if !structs.is_empty() {
        format!("{} data structures", lang_name)
    } else if !functions.is_empty() {
        format!("{} functions", lang_name)
    } else {
        format!("{} code", lang_name)
    };

    let components: Vec<String> = [
        (!functions.is_empty()).then(|| format!("{} fn", functions.len())),
        (!structs.is_empty()).then(|| format!("{} struct", structs.len())),
        (!traits.is_empty()).then(|| format!("{} trait", traits.len())),
    ]
    .into_iter()
    .flatten()
    .collect();

    let line1 = if components.is_empty() {
        format!("{} ({} lines)", main_type, total_lines)
    } else {
        format!(
            "{} ({}) - {} lines",
            main_type,
            components.join(", "),
            total_lines
        )
    };

    // Build line 2: Key details
    let mut details = Vec::new();

    // Main imports/dependencies
    if !imports.is_empty() {
        let key_imports: Vec<&str> = imports.iter().take(3).map(|s| s.as_str()).collect();
        details.push(format!("uses: {}", key_imports.join(", ")));
    }

    // Key patterns detected
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }

    // Main functions/structs
    if !functions.is_empty() {
        let key_fns: Vec<&str> = functions.iter().take(3).map(|s| s.as_str()).collect();
        if details.is_empty() {
            details.push(format!("defines: {}", key_fns.join(", ")));
        }
    }

    let line2 = if details.is_empty() {
        "General purpose code file".to_string()
    } else {
        details.join(" | ")
    };

    CodeSummary { line1, line2 }
}

fn lang_display_name(lang: &Language) -> &'static str {
    match lang {
        Language::Rust => "Rust",
        Language::Python => "Python",
        Language::JavaScript => "JavaScript",
        Language::TypeScript => "TypeScript",
        Language::Go => "Go",
        Language::C => "C",
        Language::Cpp => "C++",
        Language::Java => "Java",
        Language::Ruby => "Ruby",
        Language::Shell => "Shell",
        Language::Unknown => "Code",
    }
}

fn extract_imports(content: &str, lang: &Language) -> Vec<String> {
    let pattern = match lang {
        Language::Rust => r"^use\s+([a-zA-Z_][a-zA-Z0-9_]*(?:::[a-zA-Z_][a-zA-Z0-9_]*)?)",
        Language::Python => r"^(?:from\s+(\S+)|import\s+(\S+))",
        Language::JavaScript | Language::TypeScript => {
            r#"(?:import.*from\s+['"]([^'"]+)['"]|require\(['"]([^'"]+)['"]\))"#
        }
        Language::Go => r#"^\s*"([^"]+)"$"#,
        _ => return Vec::new(),
    };

    let re = Regex::new(pattern).unwrap();
    let mut imports = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in content.lines() {
        if let Some(caps) = re.captures(line) {
            let import = caps.get(1).or(caps.get(2)).map(|m| m.as_str().to_string());
            if let Some(imp) = import {
                let base = imp.split("::").next().unwrap_or(&imp).to_string();
                if !seen.contains(&base) && !is_std_import(&base, lang) {
                    seen.insert(base.clone());
                    imports.push(base);
                }
            }
        }
    }

    imports.into_iter().take(5).collect()
}

fn is_std_import(name: &str, lang: &Language) -> bool {
    match lang {
        Language::Rust => matches!(name, "std" | "core" | "alloc"),
        Language::Python => matches!(name, "os" | "sys" | "re" | "json" | "typing"),
        _ => false,
    }
}

fn extract_functions(content: &str, lang: &Language) -> Vec<String> {
    let pattern = match lang {
        Language::Rust => r"(?:pub\s+)?(?:async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::Python => r"def\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::JavaScript | Language::TypeScript => {
            r"(?:async\s+)?function\s+([a-zA-Z_][a-zA-Z0-9_]*)|(?:const|let|var)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*(?:async\s+)?\("
        }
        Language::Go => r"func\s+(?:\([^)]+\)\s+)?([a-zA-Z_][a-zA-Z0-9_]*)",
        _ => return Vec::new(),
    };

    let re = Regex::new(pattern).unwrap();
    let mut functions = Vec::new();

    for line in content.lines() {
        if let Some(caps) = re.captures(line) {
            let name = caps.get(1).or(caps.get(2)).map(|m| m.as_str().to_string());
            if let Some(n) = name {
                if !n.starts_with("test_") && n != "main" && n != "new" {
                    functions.push(n);
                }
            }
        }
    }

    functions.into_iter().take(10).collect()
}

fn extract_structs(content: &str, lang: &Language) -> Vec<String> {
    let pattern = match lang {
        Language::Rust => r"(?:pub\s+)?(?:struct|enum)\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::Python => r"class\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::TypeScript => r"(?:interface|class|type)\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::Go => r"type\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+struct",
        Language::Java => r"(?:public\s+)?class\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        _ => return Vec::new(),
    };

    let re = Regex::new(pattern).unwrap();
    re.captures_iter(content)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .take(10)
        .collect()
}

fn extract_traits(content: &str, lang: &Language) -> Vec<String> {
    let pattern = match lang {
        Language::Rust => r"(?:pub\s+)?trait\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        Language::TypeScript => r"interface\s+([a-zA-Z_][a-zA-Z0-9_]*)",
        _ => return Vec::new(),
    };

    let re = Regex::new(pattern).unwrap();
    re.captures_iter(content)
        .filter_map(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        .take(5)
        .collect()
}

fn detect_patterns(content: &str, lang: &Language) -> Vec<String> {
    let mut patterns = Vec::new();

    // Common patterns
    if content.contains("async") && content.contains("await") {
        patterns.push("async".to_string());
    }

    match lang {
        Language::Rust => {
            if content.contains("impl") && content.contains("for") {
                patterns.push("trait impl".to_string());
            }
            if content.contains("#[derive") {
                patterns.push("derive".to_string());
            }
            if content.contains("Result<") || content.contains("anyhow::") {
                patterns.push("error handling".to_string());
            }
            if content.contains("#[test]") {
                patterns.push("tests".to_string());
            }
            if content.contains("Box<dyn") || content.contains("&dyn") {
                patterns.push("dyn dispatch".to_string());
            }
        }
        Language::Python => {
            if content.contains("@dataclass") {
                patterns.push("dataclass".to_string());
            }
            if content.contains("def __init__") {
                patterns.push("OOP".to_string());
            }
        }
        Language::JavaScript | Language::TypeScript => {
            if content.contains("useState") || content.contains("useEffect") {
                patterns.push("React hooks".to_string());
            }
            if content.contains("export default") {
                patterns.push("ES modules".to_string());
            }
        }
        _ => {}
    }

    patterns.into_iter().take(3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_analysis() {
        let code = r#"
use anyhow::Result;
use std::fs;

pub struct Config {
    name: String,
}

pub fn load_config() -> Result<Config> {
    Ok(Config { name: "test".into() })
}

fn helper() {}
"#;
        let summary = analyze_code(code, &Language::Rust);
        assert!(summary.line1.contains("Rust"));
        assert!(summary.line1.contains("fn"));
    }

    #[test]
    fn test_python_analysis() {
        let code = r#"
import json
from pathlib import Path

class Config:
    def __init__(self, name):
        self.name = name

def load_config():
    return Config("test")
"#;
        let summary = analyze_code(code, &Language::Python);
        assert!(summary.line1.contains("Python"));
    }
}

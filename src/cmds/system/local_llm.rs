//! Summarizes source files using heuristic analysis — no external model needed.

use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::core::filter::Language;

const LUA_FUNCTION_DISPLAY_LIMIT: usize = 10;
const LUA_DETAIL_LIMIT: usize = 3;

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

    if matches!(lang, Language::Lua) {
        return analyze_lua_code(content, total_lines);
    }

    // Extract components
    let imports = extract_imports(content, lang);
    let functions = extract_functions(content, lang);
    let modules = extract_modules(content, lang);
    let structs = extract_structs(content, lang);
    let traits = extract_traits(content, lang);
    let specs = extract_specs(content, lang);

    // Detect patterns
    let patterns = detect_patterns(content, lang);

    // Build line 1: What it is
    let lang_name = lang_display_name(lang);
    let main_type = if !modules.is_empty() || (!structs.is_empty() && !functions.is_empty()) {
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
        (!modules.is_empty()).then(|| format!("{} module", modules.len())),
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

    if !modules.is_empty() {
        let key_modules: Vec<&str> = modules.iter().take(2).map(|s| s.as_str()).collect();
        details.push(format!("module: {}", key_modules.join(", ")));
    }

    // Key patterns detected
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }

    if !specs.is_empty() {
        let key_specs: Vec<&str> = specs.iter().take(3).map(|s| s.as_str()).collect();
        details.push(format!("specs: {}", key_specs.join(", ")));
    }

    // Main functions/structs. Lua summaries are for agent triage, so keep
    // entry points visible even when imports and patterns already exist.
    if !functions.is_empty() && (matches!(lang, Language::Lua) || details.is_empty()) {
        let key_fns: Vec<&str> = functions.iter().take(3).map(|s| s.as_str()).collect();
        details.push(format!("defines: {}", key_fns.join(", ")));
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
        Language::Lua => "Lua",
        Language::Shell => "Shell",
        Language::Data => "Data",
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
        Language::Lua => r#"require\s*\(?\s*['"]([^'"]+)['"]"#,
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
    if matches!(lang, Language::Lua) {
        return extract_lua_functions(content);
    }

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

struct LuaFunction {
    name: String,
    is_public: bool,
}

struct LuaTables {
    exports: Vec<String>,
    locals: Vec<String>,
}

struct LuaSpecs {
    targets: Vec<String>,
    behaviors: Vec<String>,
}

struct LuaDeps {
    internal: Vec<String>,
    external: Vec<String>,
}

fn analyze_lua_code(content: &str, total_lines: usize) -> CodeSummary {
    let specs = extract_lua_specs(content);
    let tables = extract_lua_tables(content);
    let functions = extract_lua_function_entries(content, &tables.exports);
    let deps = classify_lua_deps(content);
    let patterns = detect_patterns(content, &Language::Lua);

    if !specs.targets.is_empty() || !specs.behaviors.is_empty() {
        return build_lua_spec_summary(total_lines, &deps, &patterns, &specs);
    }

    let main_type = if !tables.exports.is_empty() {
        "Lua module"
    } else if !functions.is_empty() {
        "Lua functions"
    } else {
        "Lua code"
    };

    let mut components = Vec::new();
    if !functions.is_empty() {
        components.push(format_lua_count(
            functions.len(),
            LUA_FUNCTION_DISPLAY_LIMIT,
            "fn",
        ));
    }
    if !tables.exports.is_empty() {
        components.push(format_lua_count(tables.exports.len(), 1, "export"));
    }

    let line1 = if components.is_empty() {
        format!("{} ({} lines)", main_type, total_lines)
    } else {
        format!("{} ({}) - {} lines", main_type, components.join(", "), total_lines)
    };

    let mut details = Vec::new();
    if let Some(uses) = format_lua_deps(&deps) {
        details.push(uses);
    }
    if !tables.exports.is_empty() {
        details.push(format!(
            "exports: {}",
            tables
                .exports
                .iter()
                .take(2)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !tables.locals.is_empty() {
        details.push(format!(
            "locals: {}",
            tables
                .locals
                .iter()
                .take(LUA_DETAIL_LIMIT)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }
    if !functions.is_empty() {
        details.push(format!(
            "defines: {}",
            select_lua_function_names(&functions, LUA_DETAIL_LIMIT).join(", ")
        ));
    }

    CodeSummary {
        line1,
        line2: if details.is_empty() {
            "General purpose code file".to_string()
        } else {
            details.join(" | ")
        },
    }
}

fn build_lua_spec_summary(
    total_lines: usize,
    deps: &LuaDeps,
    patterns: &[String],
    specs: &LuaSpecs,
) -> CodeSummary {
    let examples = specs.behaviors.len();
    let line1 = if examples == 0 {
        format!("Lua spec ({} lines)", total_lines)
    } else {
        format!(
            "Lua spec ({} {}) - {} lines",
            examples,
            if examples == 1 { "example" } else { "examples" },
            total_lines
        )
    };

    let mut details = Vec::new();
    if let Some(uses) = format_lua_deps(deps) {
        details.push(uses);
    }
    if !patterns.is_empty() {
        details.push(format!("patterns: {}", patterns.join(", ")));
    }
    if let Some(target) = specs.targets.first() {
        let behaviors = specs
            .behaviors
            .iter()
            .take(LUA_DETAIL_LIMIT)
            .map(|s| s.as_str())
            .collect::<Vec<_>>();
        if behaviors.is_empty() {
            details.push(format!("specs: {}", target));
        } else {
            details.push(format!("specs: {}: {}", target, behaviors.join(", ")));
        }
    }

    CodeSummary {
        line1,
        line2: if details.is_empty() {
            "General purpose code file".to_string()
        } else {
            details.join(" | ")
        },
    }
}

fn format_lua_count(count: usize, cap: usize, label: &str) -> String {
    if count > cap {
        format!("{}+ {}", cap, label)
    } else {
        format!("{} {}", count, label)
    }
}

fn extract_lua_functions(content: &str) -> Vec<String> {
    extract_lua_function_entries(content, &[])
        .into_iter()
        .map(|f| f.name)
        .take(LUA_FUNCTION_DISPLAY_LIMIT)
        .collect()
}

fn extract_lua_function_entries(content: &str, exports: &[String]) -> Vec<LuaFunction> {
    let declaration_re = Regex::new(
        r"^\s*(local\s+)?function\s+(?:([A-Za-z_][A-Za-z0-9_]*)[.:])?([A-Za-z_][A-Za-z0-9_]*)",
    )
    .unwrap();
    let assignment_re = Regex::new(
        r"^\s*(?:(local\s+)?([A-Za-z_][A-Za-z0-9_]*)|([A-Za-z_][A-Za-z0-9_]*)[.:]([A-Za-z_][A-Za-z0-9_]*))\s*=\s*function\s*\(",
    )
    .unwrap();
    let export_set: std::collections::HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    let mut functions = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in content.lines() {
        let parsed = declaration_re.captures(line).and_then(|caps| {
            let is_local = caps.get(1).is_some();
            let owner = caps.get(2).map(|m| m.as_str().to_string());
            let name = caps.get(3)?.as_str().to_string();
            Some((name, owner, !is_local))
        })
        .or_else(|| {
            assignment_re.captures(line).and_then(|caps| {
                let is_local = caps.get(1).is_some();
                let local_name = caps.get(2).map(|m| m.as_str().to_string());
                let owner = caps.get(3).map(|m| m.as_str().to_string());
                let method_name = caps.get(4).map(|m| m.as_str().to_string());
                let name = method_name.or(local_name)?;
                Some((name, owner, !is_local))
            })
        });

        if let Some((name, owner, public_without_owner)) = parsed {
            if name.starts_with("test_") || name == "main" {
                continue;
            }
            if seen.insert(name.clone()) {
                let is_public = owner
                    .as_deref()
                    .map(|owner| export_set.contains(owner) || is_public_lua_owner(owner))
                    .unwrap_or(public_without_owner);
                functions.push(LuaFunction { name, is_public });
            }
        }
    }

    functions
}

fn extract_modules(content: &str, lang: &Language) -> Vec<String> {
    if matches!(lang, Language::Lua) {
        extract_lua_tables(content).exports
    } else {
        Vec::new()
    }
}

fn extract_lua_tables(content: &str) -> LuaTables {
    let table_re = Regex::new(
        r"^\s*(?:local\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:\{|setmetatable\s*\()",
    )
    .unwrap();
    let return_re = Regex::new(r"^\s*return\s+([A-Za-z_][A-Za-z0-9_]*)\s*$").unwrap();
    let mut tables = Vec::new();
    let mut seen_tables = std::collections::HashSet::new();
    let mut returned = std::collections::HashSet::new();

    for line in content.lines() {
        if let Some(name) = table_re
            .captures(line)
            .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        {
            if seen_tables.insert(name.clone()) {
                tables.push(name);
            }
        }

        if let Some(name) = return_re
            .captures(line)
            .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
        {
            returned.insert(name);
        }
    }

    let exports = tables
        .iter()
        .filter(|name| returned.contains(*name))
        .cloned()
        .collect::<Vec<_>>();
    let export_set: std::collections::HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    let locals = tables
        .into_iter()
        .filter(|name| !export_set.contains(name.as_str()))
        .take(LUA_DETAIL_LIMIT)
        .collect();

    LuaTables { exports, locals }
}

fn is_public_lua_owner(owner: &str) -> bool {
    owner
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
}

fn select_lua_function_names(functions: &[LuaFunction], limit: usize) -> Vec<String> {
    let mut names = functions
        .iter()
        .filter(|f| f.is_public)
        .map(|f| f.name.clone())
        .take(limit)
        .collect::<Vec<_>>();

    if names.len() < limit {
        names.extend(
            functions
                .iter()
                .filter(|f| !f.is_public)
                .map(|f| f.name.clone())
                .take(limit - names.len()),
        );
    }

    names
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

fn extract_specs(content: &str, lang: &Language) -> Vec<String> {
    if !matches!(lang, Language::Lua) {
        return Vec::new();
    }

    let specs = extract_lua_specs(content);
    specs
        .targets
        .into_iter()
        .chain(specs.behaviors)
        .take(5)
        .collect()
}

fn extract_lua_specs(content: &str) -> LuaSpecs {
    let describe_re = Regex::new(r#"\bdescribe\s*\(?\s*["']([^"']+)["']"#).unwrap();
    let it_re = Regex::new(r#"\bit\s*\(?\s*["']([^"']+)["']"#).unwrap();
    let mut targets = Vec::new();
    let mut behaviors = Vec::new();
    let mut seen_targets = std::collections::HashSet::new();
    let mut seen_behaviors = std::collections::HashSet::new();

    for caps in describe_re.captures_iter(content) {
        if let Some(name) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
            if seen_targets.insert(name.clone()) {
                targets.push(name);
            }
        }
    }

    for caps in it_re.captures_iter(content) {
        if let Some(name) = caps.get(1).map(|m| m.as_str().trim().to_string()) {
            if seen_behaviors.insert(name.clone()) {
                behaviors.push(name);
            }
        }
    }

    LuaSpecs { targets, behaviors }
}

fn classify_lua_deps(content: &str) -> LuaDeps {
    let re = Regex::new(r#"require\s*\(?\s*['"]([^'"]+)['"]"#).unwrap();
    let mut internal = Vec::new();
    let mut external = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for caps in re.captures_iter(content) {
        if let Some(dep) = caps.get(1).map(|m| m.as_str().to_string()) {
            if !seen.insert(dep.clone()) {
                continue;
            }
            if is_lua_external_dep(&dep) {
                external.push(dep);
            } else {
                internal.push(dep);
            }
        }
    }

    LuaDeps { internal, external }
}

fn is_lua_external_dep(dep: &str) -> bool {
    let first = dep
        .split(['/', '.'])
        .next()
        .unwrap_or(dep)
        .to_ascii_lowercase();
    let has_namespace = dep.contains('/') || dep.contains('.');

    if !has_namespace {
        return true;
    }

    matches!(
        first.as_str(),
        "busted"
            | "cjson"
            | "dkjson"
            | "ffi"
            | "lfs"
            | "lpeg"
            | "luassert"
            | "mime"
            | "pl"
            | "resty"
            | "say"
            | "socket"
    )
}

fn format_lua_deps(deps: &LuaDeps) -> Option<String> {
    let mut parts = Vec::new();

    if !deps.internal.is_empty() {
        parts.push(format!(
            "internal: {}",
            deps.internal
                .iter()
                .take(LUA_DETAIL_LIMIT)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !deps.external.is_empty() {
        parts.push(format!(
            "external: {}",
            deps.external
                .iter()
                .take(LUA_DETAIL_LIMIT)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    (!parts.is_empty()).then(|| format!("uses: {}", parts.join(" | ")))
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
        Language::Lua => {
            if content.contains("describe(") || content.contains("it(") {
                patterns.push("busted specs".to_string());
            }
            if content.contains("setmetatable") || content.contains("__index") {
                patterns.push("class-like".to_string());
            }
            if Regex::new(r"(?m)^\s*return\s+(?:[A-Za-z_][A-Za-z0-9_]*|\{)")
                .unwrap()
                .is_match(content)
            {
                patterns.push("module export".to_string());
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

    #[test]
    fn test_lua_analysis_detects_module_assignments_and_exports() {
        let code = r#"
local Router = require("app/router")
local Store = require("app/store")

local SourceCatalog = {}

function SourceCatalog:show()
    Router:open(Store:list())
end

SourceCatalog.load = function()
    return {}
end

local build_rows = function(items)
    return items
end

return SourceCatalog
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line1.contains("Lua module"));
        assert!(summary.line1.contains("3 fn"));
        assert!(summary.line1.contains("1 export"));
        assert!(summary.line2.contains("uses: internal: app/router, app/store"));
        assert!(summary.line2.contains("exports: SourceCatalog"));
        assert!(summary.line2.contains("patterns: module export"));
        assert!(summary
            .line2
            .contains("defines: show, load, build_rows"));
    }

    #[test]
    fn test_lua_analysis_detects_busted_specs() {
        let code = r#"
local ui = require("suwayomi/ui")

describe("source browser", function()
    it("lists languages", function()
        assert.truthy(ui)
    end)
end)
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line1.contains("Lua spec"));
        assert!(summary.line1.contains("1 example"));
        assert!(!summary.line2.contains("module: assert"));
        assert!(summary.line2.contains("patterns: busted specs"));
        assert!(summary
            .line2
            .contains("specs: source browser: lists languages"));
    }

    #[test]
    fn test_lua_analysis_marks_capped_function_count() {
        let code = r#"
local LargeModule = {}

function LargeModule:one() end
function LargeModule:two() end
function LargeModule:three() end
function LargeModule:four() end
function LargeModule:five() end
function LargeModule:six() end
function LargeModule:seven() end
function LargeModule:eight() end
function LargeModule:nine() end
function LargeModule:ten() end
function LargeModule:eleven() end
function LargeModule:twelve() end

return LargeModule
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line1.contains("10+ fn"));
        assert!(!summary.line1.contains("12 fn"));
    }

    #[test]
    fn test_lua_analysis_prioritizes_public_methods_over_helpers() {
        let code = r#"
local Controller = {}

local function parse_config() end
local function normalize_path() end
local helper = function() end

function Controller:start() end
Controller.stop = function() end
function Controller.restart() end

return Controller
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary
            .line2
            .contains("defines: start, stop, restart"));
        assert!(!summary.line2.contains("defines: parse_config"));
    }

    #[test]
    fn test_lua_analysis_distinguishes_exports_from_local_tables() {
        let code = r#"
local DownloadQueue = {}
DownloadQueue.__index = DownloadQueue

local queue = {}
local menu_table = {}

function DownloadQueue:new() end
function DownloadQueue:add() end

return DownloadQueue
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line2.contains("exports: DownloadQueue"));
        assert!(summary.line2.contains("locals: queue, menu_table"));
        assert!(!summary.line2.contains("module: DownloadQueue, queue"));
    }

    #[test]
    fn test_lua_analysis_summarizes_busted_specs_over_mock_helpers() {
        let code = r#"
local settings = require("app/settings")

local function open() end
local function write() end
local function mkdir() end
local function install_file_mock() end

describe("settings controller", function()
    it("loads saved settings", function() end)
    it("falls back to defaults", function() end)
    it("persists changed values", function() end)
end)
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line1.contains("Lua spec"));
        assert!(summary.line1.contains("3 examples"));
        assert!(summary
            .line2
            .contains("specs: settings controller: loads saved settings, falls back to defaults, persists changed values"));
        assert!(!summary.line2.contains("defines:"));
        assert!(!summary.line2.contains("open"));
        assert!(!summary.line2.contains("install_file_mock"));
    }

    #[test]
    fn test_lua_analysis_classifies_require_dependencies_stably() {
        let code = r#"
local gettext = require("gettext")
local UIManager = require("ui/uimanager")
local Settings = require("app/settings")
local Routes = require("app/routes")

local App = {}
function App:start() end
return App
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line2.contains(
            "uses: internal: ui/uimanager, app/settings, app/routes | external: gettext"
        ));
    }

    #[test]
    fn test_lua_analysis_keeps_public_new_constructor() {
        let code = r#"
local Widget = {}
Widget.__index = Widget

local function prepare_options() end
function Widget:new() end
function Widget:render() end

return Widget
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line2.contains("defines: new, render"));
        assert!(!summary.line2.contains("defines: prepare_options"));
    }

    #[test]
    fn test_lua_analysis_treats_ui_namespace_as_internal_by_default() {
        let code = r#"
local UIManager = require("ui/uimanager")
local Widget = require("ui/widget/menu")
local gettext = require("gettext")

local App = {}
function App:start() end
return App
"#;

        let summary = analyze_code(code, &Language::Lua);

        assert!(summary.line2.contains("internal: ui/uimanager, ui/widget/menu"));
        assert!(summary.line2.contains("external: gettext"));
    }
}

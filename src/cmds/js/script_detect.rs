//! Detect which tool an npm script invokes by reading `package.json`.
//!
//! Adapter-time only — disk I/O is fine here (unlike the rewrite path, which
//! is on the hot hook path and must stay allocation-free). Conservative by
//! design: returns `None` whenever the script body mentions zero or multiple
//! known tools, so we never apply a tool-specific filter unless we are sure
//! the tool is what actually runs.

use serde_json::Value;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptTool {
    Biome,
    // Future additions (Eslint, Prettier, Tsc) gated on the corresponding
    // TOML or Rust filter existing in the tree.
}

/// Read `package.json` from the given directory and detect the tool invoked
/// by the named script.
pub fn detect_script_tool_in(dir: &Path, script_name: &str) -> Option<ScriptTool> {
    let raw = fs::read_to_string(dir.join("package.json")).ok()?;
    let json: Value = serde_json::from_str(&raw).ok()?;
    let script_body = json.get("scripts")?.get(script_name)?.as_str()?;
    detect_tool_in_script(script_body)
}

/// Convenience wrapper that reads `package.json` from the current working
/// directory. Kept thin so the core `_in` variant can be tested without
/// touching process-wide cwd.
pub fn detect_script_tool(script_name: &str) -> Option<ScriptTool> {
    detect_script_tool_in(Path::new("."), script_name)
}

/// Parse a script body and return the single tool it invokes, or `None`.
fn detect_tool_in_script(body: &str) -> Option<ScriptTool> {
    let has_biome = has_tool_token(body, "biome");
    let has_eslint = has_tool_token(body, "eslint");
    let has_prettier = has_tool_token(body, "prettier");
    let has_tsc = has_tool_token(body, "tsc");

    let tools_found = [has_biome, has_eslint, has_prettier, has_tsc]
        .iter()
        .filter(|b| **b)
        .count();
    if tools_found != 1 {
        return None;
    }

    if has_biome {
        Some(ScriptTool::Biome)
    } else {
        // Recognised but no filter wired yet (eslint / prettier / tsc).
        None
    }
}

/// True if `body` contains `tool` as a distinct token (not a substring of a
/// longer word). Handles whitespace, shell separators (`&`, `|`, `;`), and
/// path-prefixed forms like `./node_modules/.bin/biome` or
/// `/usr/local/bin/biome`.
fn has_tool_token(body: &str, tool: &str) -> bool {
    body.split(|c: char| c.is_whitespace() || "&|;".contains(c))
        .any(|tok| {
            let basename = tok.rsplit('/').next().unwrap_or(tok);
            basename == tool
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_biome_in_simple_script() {
        assert_eq!(detect_tool_in_script("biome check"), Some(ScriptTool::Biome));
    }

    #[test]
    fn detects_biome_with_args() {
        assert_eq!(
            detect_tool_in_script("biome check ./src --reporter=compact"),
            Some(ScriptTool::Biome)
        );
    }

    #[test]
    fn detects_biome_with_relative_path() {
        assert_eq!(
            detect_tool_in_script("./node_modules/.bin/biome check ./src"),
            Some(ScriptTool::Biome)
        );
    }

    #[test]
    fn detects_biome_with_absolute_path() {
        assert_eq!(
            detect_tool_in_script("/usr/local/bin/biome check"),
            Some(ScriptTool::Biome)
        );
    }

    #[test]
    fn returns_none_for_eslint_until_wired() {
        // Parser recognises eslint but we do not yet ship an eslint text
        // filter, so the detector is deliberately conservative here.
        assert_eq!(detect_tool_in_script("eslint src/"), None);
    }

    #[test]
    fn returns_none_for_prettier_until_wired() {
        assert_eq!(detect_tool_in_script("prettier --check ."), None);
    }

    #[test]
    fn returns_none_for_mixed_biome_and_eslint() {
        assert_eq!(
            detect_tool_in_script("biome check && eslint src/"),
            None
        );
    }

    #[test]
    fn returns_none_for_mixed_biome_and_prettier() {
        assert_eq!(
            detect_tool_in_script("biome check; prettier --check ."),
            None
        );
    }

    #[test]
    fn returns_none_for_unknown_script() {
        assert_eq!(detect_tool_in_script("bash ./scripts/lint.sh"), None);
    }

    #[test]
    fn returns_none_for_empty_script() {
        assert_eq!(detect_tool_in_script(""), None);
    }

    #[test]
    fn does_not_match_substring_biome() {
        // Words like `prebiome` or `biomejs` must not trigger biome detection.
        assert_eq!(detect_tool_in_script("prebiome --check"), None);
        assert_eq!(detect_tool_in_script("biomejs check"), None);
    }

    #[test]
    fn does_not_match_substring_eslint() {
        // Words like `my-eslint-wrapper` must not trigger eslint detection.
        assert_eq!(detect_tool_in_script("my-eslint-wrapper"), None);
    }

    #[test]
    fn handles_pipe_separator() {
        // `|` is a shell separator; tokens on either side should be split.
        assert_eq!(
            detect_tool_in_script("biome check | tee out.log"),
            Some(ScriptTool::Biome)
        );
    }

    #[test]
    fn reads_package_json_from_given_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts": {"lint": "biome check", "build": "tsc"}}"#,
        )
        .unwrap();
        assert_eq!(
            detect_script_tool_in(tmp.path(), "lint"),
            Some(ScriptTool::Biome)
        );
    }

    #[test]
    fn returns_none_for_missing_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_script_tool_in(tmp.path(), "lint"), None);
    }

    #[test]
    fn returns_none_for_missing_script_name() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts": {"build": "tsc"}}"#,
        )
        .unwrap();
        assert_eq!(detect_script_tool_in(tmp.path(), "lint"), None);
    }

    #[test]
    fn returns_none_for_malformed_package_json() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("package.json"), "not valid json {").unwrap();
        assert_eq!(detect_script_tool_in(tmp.path(), "lint"), None);
    }

    #[test]
    fn returns_none_when_scripts_field_is_not_an_object() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts": "not-an-object"}"#,
        )
        .unwrap();
        assert_eq!(detect_script_tool_in(tmp.path(), "lint"), None);
    }
}

//! Compact the native property-list output of `xcrun simctl listapps`.

use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::args_utils::restore_double_dash;
use crate::core::utils::{exit_code_from_output, resolved_command};
use crate::core::{guard, runner, tee, tracking};
use anyhow::{Context, Result};
use std::borrow::Cow;
use std::io::{self, Write};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = restore_double_dash(args);
    if !is_app_inventory(&args) {
        let args: Vec<std::ffi::OsString> = args.iter().map(Into::into).collect();
        return runner::run_passthrough("xcrun", &args, verbose);
    }
    if verbose > 0 {
        eprintln!("Running: xcrun {}", args.join(" "));
    }
    // Capture bytes directly: the shared line-oriented runner normalizes
    // unterminated lines, but an unrecognized plist must fall back byte-for-byte.
    let timer = tracking::TimedExecution::start();
    let output = resolved_command("xcrun")
        .args(&args)
        .output()
        .context("Failed to run xcrun simctl listapps")?;
    let exit_code = exit_code_from_output(&output, "xcrun");
    let shown = compact_stdout(&output.stdout, exit_code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", String::from_utf8_lossy(&output.stdout), stderr);
    let emitted = format!("{}{}", String::from_utf8_lossy(&shown), stderr);
    let command = format!("xcrun {}", args.join(" "));
    timer.track(&command, &format!("rtk {command}"), &raw, &emitted);
    io::stdout()
        .write_all(&shown)
        .context("Failed to write simctl stdout")?;
    io::stderr()
        .write_all(&output.stderr)
        .context("Failed to write simctl stderr")?;
    Ok(exit_code)
}

fn compact_stdout(raw: &[u8], exit_code: i32) -> Cow<'_, [u8]> {
    if exit_code != 0 || raw.is_empty() {
        return Cow::Borrowed(raw);
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        return Cow::Borrowed(raw);
    };
    let Some(mut filtered) = filter_listapps(text) else {
        eprintln!("rtk: unrecognized simctl app inventory; preserving raw output");
        return Cow::Borrowed(raw);
    };
    if let Some(hint) = tee::tee_and_hint(text, "simctl_listapps", exit_code) {
        filtered.push_str(&format!("\n{hint}\n"));
    }
    Cow::Owned(guard::never_worse(text, &filtered).as_bytes().to_vec())
}

fn is_app_inventory(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(
        args,
        &|kind, name| match (kind, name) {
            (TokenKind::Long, "sdk" | "toolchain" | "set" | "profiles") => Some(ValueSpec::value()),
            _ => None,
        },
        Dialect::Posix,
    );
    // Only the plain inventory has a known output contract. Explicit options,
    // xcrun lookup modes, and all other tools/subcommands retain native stdio.
    tokens.len() == 3
        && tokens.iter().all(|token| token.is_free_positional())
        && tokens[0].text == "simctl"
        && tokens[1].text == "listapps"
        && !tokens[2].text.is_empty()
}

fn filter_listapps(raw: &str) -> Option<String> {
    let apps = Plist::parse(raw)?;
    if apps.is_empty() {
        return Some(raw.to_string());
    }
    let mut output = String::from("{\n");
    for (bundle_id, value) in apps {
        let fields = Plist::parse(value)?;
        // An unfamiliar app schema must not silently turn into an empty record.
        for required in ["CFBundleIdentifier", "Path"] {
            if !fields
                .iter()
                .any(|(key, _)| key.trim_matches('"') == required)
            {
                return None;
            }
        }
        output.push_str(&format!("    {bundle_id} = {{\n"));
        for (key, value) in fields {
            if matches!(
                key.trim_matches('"'),
                "ApplicationType"
                    | "CFBundleDisplayName"
                    | "CFBundleName"
                    | "CFBundleIdentifier"
                    | "CFBundleShortVersionString"
                    | "CFBundleVersion"
                    | "Path"
                    | "DTSDKName"
                    | "SDK"
            ) {
                output.push_str(&format!("        {key} = {value};\n"));
            }
        }
        output.push_str("    };\n");
    }
    output.push_str("}\n");
    Some(output)
}

/// Read the OpenStep text emitted by simctl without decoding/re-encoding strings.
/// Keeping original spans preserves escaping, Unicode, and full paths. Unknown
/// syntax fails closed to raw output; this is not a general-purpose plist codec.
struct Plist<'a> {
    text: &'a str,
    pos: usize,
}

type Entries<'a> = Vec<(&'a str, &'a str)>;

impl<'a> Plist<'a> {
    fn parse(text: &'a str) -> Option<Entries<'a>> {
        let mut parser = Self { text, pos: 0 };
        let entries = parser.dictionary(0)?;
        parser.whitespace();
        (parser.pos == text.len()).then_some(entries)
    }

    fn whitespace(&mut self) {
        while self
            .text
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
    }

    fn consume(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.text.as_bytes().get(self.pos) == Some(&byte) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn scalar(&mut self) -> Option<&'a str> {
        self.whitespace();
        let start = self.pos;
        if self.consume(b'"') {
            while let Some(&byte) = self.text.as_bytes().get(self.pos) {
                self.pos += 1;
                match byte {
                    b'\\' => {
                        self.text.as_bytes().get(self.pos)?;
                        self.pos += 1;
                    }
                    b'"' => return Some(&self.text[start..self.pos]),
                    _ => {}
                }
            }
            return None;
        }
        while let Some(&byte) = self.text.as_bytes().get(self.pos) {
            if byte.is_ascii_whitespace() || b"{}()=;,".contains(&byte) {
                break;
            }
            // Data literals and comments aren't present in known simctl output.
            if b"<>\"".contains(&byte)
                || self.text.as_bytes()[self.pos..].starts_with(b"/*")
                || self.text.as_bytes()[self.pos..].starts_with(b"//")
            {
                return None;
            }
            self.pos += 1;
        }
        (self.pos > start).then(|| &self.text[start..self.pos])
    }

    fn dictionary(&mut self, depth: usize) -> Option<Entries<'a>> {
        if depth > 32 || !self.consume(b'{') {
            return None;
        }
        let mut entries = Vec::new();
        while !self.consume(b'}') {
            let key = self.scalar()?;
            if !self.consume(b'=') {
                return None;
            }
            let value = self.value(depth + 1)?;
            if !self.consume(b';') {
                return None;
            }
            entries.push((key, value));
        }
        Some(entries)
    }

    fn value(&mut self, depth: usize) -> Option<&'a str> {
        if depth > 32 {
            return None;
        }
        self.whitespace();
        let start = self.pos;
        match self.text.as_bytes().get(self.pos)? {
            b'{' => {
                self.dictionary(depth)?;
            }
            b'(' => {
                self.pos += 1;
                while !self.consume(b')') {
                    self.value(depth + 1)?;
                    if self.consume(b')') {
                        break;
                    }
                    if !self.consume(b',') {
                        return None;
                    }
                }
            }
            _ => {
                self.scalar()?;
            }
        }
        Some(&self.text[start..self.pos])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    const RAW: &str = include_str!("../../../tests/fixtures/simctl_listapps_raw.txt");

    #[test]
    fn real_inventory_preserves_native_fields_and_saves_tokens() {
        let filtered = filter_listapps(RAW).expect("recognized inventory");
        assert_eq!(
            filtered,
            include_str!("../../../tests/fixtures/simctl_listapps_expected.txt")
        );
        assert!(estimate_tokens(&filtered) * 100 < estimate_tokens(RAW) * 40);
    }

    #[test]
    fn quoted_delimiters_nested_metadata_and_unicode_names_are_safe() {
        let raw = r#"{
    "com.example.Camera" = {
        CFBundleIdentifier = "com.example.Camera";
        CFBundleName = "Cámara \"{test};\"";
        Path = "/Applications/Camera (test).app";
        Metadata = { names = ("} ; = {", { value = "\\"; }); };
    };
}"#;
        let filtered = filter_listapps(raw).expect("valid property list");
        assert!(filtered.contains(r#"CFBundleName = "Cámara \"{test};\"";"#));
        assert!(filtered.contains(r#"Path = "/Applications/Camera (test).app";"#));
        assert!(!filtered.contains("Metadata"));
    }

    #[test]
    fn unquoted_unicode_scalar_is_preserved() {
        let raw = "{app = { CFBundleIdentifier = app; CFBundleName = Cámara; Path = /tmp/app; };}";
        assert!(
            filter_listapps(raw)
                .expect("unicode scalar")
                .contains("CFBundleName = Cámara;")
        );
    }

    #[test]
    fn empty_inventory_stays_empty() {
        for raw in ["{}", "{\n}\n"] {
            assert_eq!(filter_listapps(raw).as_deref(), Some(raw));
        }
    }

    #[test]
    fn malformed_unknown_or_truncated_output_falls_back() {
        for raw in [
            "",
            "No devices are booted.",
            "{",
            "{} trailing text",
            r#"{"app" = { CFBundleIdentifier = app; Path = "/x"; };"#,
            r#"{"app" = { CFBundleIdentifier = app; Path = "/x; };}"#,
            r#"{"app" = { CFBundleIdentifier = app; Path = "/x"; broken };}"#,
            r#"{"app" = { CFBundleIdentifier = app; };}"#,
            r#"{"app" = { Path = "/x"; };}"#,
            r#"{"app" = "unexpected";}"#,
            r#"{"app" = { CFBundleIdentifier = app; Path = "/x"; data = <01ff>; };}"#,
        ] {
            assert!(filter_listapps(raw).is_none(), "{raw}");
        }
        let nested = format!("{{x={};}}", "(".repeat(1000));
        assert!(filter_listapps(&nested).is_none());
    }

    #[test]
    fn only_plain_app_inventory_is_filtered() {
        let args = ["simctl", "listapps", "booted"].map(String::from);
        assert!(is_app_inventory(&args));
        for args in [
            vec!["simctl", "listapps"],
            vec!["simctl", "listapps", "booted", "-v"],
            vec!["simctl", "listapps", "--", "booted"],
            vec!["simctl", "list", "devices"],
            vec!["simctl", "spawn", "booted"],
            vec!["--sdk", "simctl", "listapps", "booted"],
            vec!["--find", "simctl", "listapps", "booted"],
            vec!["clang", "listapps", "booted"],
        ] {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            assert!(!is_app_inventory(&args), "{args:?}");
        }
    }
}

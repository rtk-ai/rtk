//! Filters MSBuild output — keeps cl/link diagnostics, drops project/task noise.
//!
//! Captures stdout AND stderr (default for run_filtered) so linker errors from
//! `link.exe` (which writes to stderr) survive into the filter input.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;

lazy_static! {
    // Compiler: file(line): error|warning C1234: message [project.vcxproj]
    static ref MSVC_COMPILER_RE: Regex =
        Regex::new(r"^(.+)\((\d+)\): (error|warning|fatal error) (C\d+): (.+?)(?:\s+\[.+\])?$")
            .unwrap();
    // Linker: module : error|fatal error LNK1234: message
    static ref MSVC_LINKER_RE: Regex =
        Regex::new(r"^(.+) : (error|fatal error) (LNK\d+): (.+)$").unwrap();
    // Linker tool (no file prefix): "LINK : fatal error LNK1104: ..."
    static ref MSVC_LINK_TOOL_RE: Regex =
        Regex::new(r"^(?i:LINK)\s*: (warning|error|fatal error) (LNK\d+): (.+)$").unwrap();
    // Resource compiler: file.rc(line): error|fatal error RC1234: message [project.vcxproj]
    static ref RC_DIAG_RE: Regex =
        Regex::new(r"^(.+)\((\d+)\): (warning|error|fatal error) (RC\d+): (.+?)(?:\s+\[.+\])?$")
            .unwrap();
    // MSBuild-style diagnostics: path.vcxproj(123,5): error MSB3073: ...
    static ref MSBUILD_DIAG_RE: Regex = Regex::new(
        r"^(.+?)\((\d+)(?:,(\d+))?\): (warning|error|fatal error) ((?:MSB|PRJ|CVT|LNK|RC|C)\d+): (.+)$"
    )
    .unwrap();
    static ref MSB3073_RE: Regex = Regex::new(r"(?i)\b(MSB3073|MSB3721)\b").unwrap();
    static ref EXIT_CODE_RE: Regex = Regex::new(r"(?i)\bexited with code\s+(\d+)\b").unwrap();
    static ref PROJECT_ON_NODE_RE: Regex =
        Regex::new(r#"^Project \"(.+?)\" on node \d+ \((.+?) target\(s\)\)\."#).unwrap();
    static ref DONE_BUILDING_RE: Regex =
        Regex::new(r#"^Done Building Project \"(.+?)\" \(.+\) -- (FAILED|SUCCESSFUL)\."#)
            .unwrap();
    // Build FAILED. or Build succeeded.
    static ref BUILD_RESULT_RE: Regex = Regex::new(r"^Build (FAILED|succeeded)\.").unwrap();
    // "    N Error(s)" or "    N Warning(s)"
    static ref ERR_WARN_COUNT_RE: Regex = Regex::new(r"^\s+\d+\s+(Error|Warning)\(s\)").unwrap();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct MsbuildDiag {
    idx: usize,
    severity: Severity,
    code: String,
    file: Option<String>,
    line: Option<usize>,
    message: String,
    project: Option<String>,
    raw: String,
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut adjusted: Vec<String> = Vec::with_capacity(args.len() + 1);
    let mut found_link_only = false;
    for a in args {
        // Always rewrite /t:Link-only to /t:Build to capture both compile + link output
        let lower = a.to_ascii_lowercase();
        if lower == "/t:link" || lower == "-t:link" {
            adjusted.push("/t:Build".to_string());
            found_link_only = true;
        } else {
            adjusted.push(a.clone());
        }
    }
    if verbose > 0 && found_link_only {
        eprintln!("rtk msbuild: rewrote /t:Link → /t:Build to capture linker output");
    }

    let mut cmd = resolved_command("msbuild");
    for a in &adjusted {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: msbuild {}", adjusted.join(" "));
    }

    let args_owned = adjusted.clone();
    runner::run_filtered(
        cmd,
        "msbuild",
        &adjusted.join(" "),
        move |raw| filter_output(raw, &args_owned),
        RunOptions::with_tee("msbuild"),
    )
}

pub(crate) fn filter_output(raw: &str, args: &[String]) -> String {
    let mut diags: Vec<MsbuildDiag> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    let mut build_result: Option<String> = None;
    let mut succeeded = false;
    let mut current_project: Option<String> = None;
    let mut failed_projects: Vec<String> = Vec::new();

    let lines: Vec<&str> = raw.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = PROJECT_ON_NODE_RE.captures(trimmed) {
            current_project = Some(caps[1].to_string());
            continue;
        }
        if let Some(caps) = DONE_BUILDING_RE.captures(trimmed) {
            let proj = caps[1].to_string();
            let status = &caps[2];
            if status.eq_ignore_ascii_case("FAILED") && !failed_projects.contains(&proj) {
                failed_projects.push(proj.clone());
            }
            current_project = Some(proj);
            continue;
        }

        if let Some(diag) = parse_diag_line(trimmed, idx, current_project.as_deref()) {
            diags.push(diag);
            continue;
        }
        if let Some(caps) = BUILD_RESULT_RE.captures(trimmed) {
            build_result = Some(trimmed.to_string());
            succeeded = &caps[1] == "succeeded";
            continue;
        }
        if ERR_WARN_COUNT_RE.is_match(trimmed) {
            summary.push(trimmed.to_string());
            continue;
        }
    }

    let has_errors = diags.iter().any(|d| d.severity == Severity::Error);

    if !has_errors && build_result.is_none() && diags.is_empty() {
        // Empty / redirected output
        let target = configuration_summary(args);
        return format!(
            "msbuild: no output captured \u{2014} rerun with /t:Build to capture linker output\n\
             [target: {}]",
            target
        );
    }

    if succeeded && !has_errors {
        let target = configuration_summary(args);
        return format!("msbuild: ok  {}", target);
    }

    let mut out = String::new();

    if let Some(first_error) = first_real_error(&diags) {
        let target = configuration_summary(args);
        out.push_str("FIRST_ERROR\n");
        if !target.is_empty() {
            out.push_str(&format!("  target: {}\n", target));
        }
        if let Some(p) = first_error.project.as_deref() {
            out.push_str(&format!("  project: {}\n", p));
        }
        if let Some(f) = first_error.file.as_deref() {
            out.push_str(&format!("  file: {}\n", f));
        }
        if let Some(ln) = first_error.line {
            out.push_str(&format!("  line: {}\n", ln));
        }
        out.push_str(&format!("  code: {}\n", first_error.code));
        out.push_str(&format!("  message: {}\n", first_error.message));

        let ctx = extract_context(&lines, first_error.idx, 3, 5);
        if !ctx.prev.is_empty() || !ctx.next.is_empty() {
            out.push_str("  context:\n");
            for l in ctx.prev {
                out.push_str(&format!("    - {}\n", l));
            }
            for l in ctx.next {
                out.push_str(&format!("    + {}\n", l));
            }
        }
        out.push('\n');
    }

    if !failed_projects.is_empty() {
        out.push_str("FAILED_PROJECTS\n");
        for p in &failed_projects {
            out.push_str(&format!("  - {}\n", p));
        }
        out.push('\n');
    }

    out.push_str("DIAGNOSTICS\n");
    for d in dedup_diags(&diags)
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
    {
        out.push_str(&d.raw);
        out.push('\n');
    }
    if !succeeded {
        for d in dedup_diags(&diags)
            .into_iter()
            .filter(|d| d.severity == Severity::Warning)
        {
            out.push_str(&d.raw);
            out.push('\n');
        }
    }
    if let Some(br) = build_result {
        out.push_str(&br);
        out.push('\n');
    }
    for s in &summary {
        out.push_str(s);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn parse_diag_line(line: &str, idx: usize, current_project: Option<&str>) -> Option<MsbuildDiag> {
    // Extract the project path from trailing "[...vcxproj]" when present.
    let project_from_suffix = line
        .rfind('[')
        .and_then(|i| line[i..].strip_prefix('['))
        .and_then(|rest| rest.strip_suffix(']'))
        .map(|s| s.trim().to_string());
    let project = project_from_suffix.or_else(|| current_project.map(str::to_string));

    if let Some(caps) = MSVC_COMPILER_RE.captures(line) {
        let file = caps.get(1)?.as_str().to_string();
        let lnum: usize = caps.get(2)?.as_str().parse().ok()?;
        let kind = caps.get(3)?.as_str();
        let code = caps.get(4)?.as_str().to_string();
        let msg = caps.get(5)?.as_str().to_string();
        let severity = if kind.eq_ignore_ascii_case("warning") {
            Severity::Warning
        } else {
            Severity::Error
        };
        let raw = format!("{}({}): {} {}: {}", file, lnum, kind, code, msg);
        return Some(MsbuildDiag {
            idx,
            severity,
            code,
            file: Some(file),
            line: Some(lnum),
            message: msg,
            project,
            raw,
        });
    }

    if let Some(caps) = RC_DIAG_RE.captures(line) {
        let file = caps.get(1)?.as_str().to_string();
        let lnum: usize = caps.get(2)?.as_str().parse().ok()?;
        let kind = caps.get(3)?.as_str();
        let code = caps.get(4)?.as_str().to_string();
        let msg = caps.get(5)?.as_str().to_string();
        let severity = if kind.eq_ignore_ascii_case("warning") {
            Severity::Warning
        } else {
            Severity::Error
        };
        return Some(MsbuildDiag {
            idx,
            severity,
            code,
            file: Some(file),
            line: Some(lnum),
            message: msg,
            project,
            raw: line.to_string(),
        });
    }

    if let Some(caps) = MSBUILD_DIAG_RE.captures(line) {
        let file = caps.get(1)?.as_str().to_string();
        let lnum: usize = caps.get(2)?.as_str().parse().ok()?;
        let kind = caps.get(4)?.as_str();
        let code = caps.get(5)?.as_str().to_string();
        let mut msg = caps.get(6)?.as_str().to_string();
        if MSB3073_RE.is_match(&code) {
            let exit_code = EXIT_CODE_RE
                .captures(&msg)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string());
            if let Some(cmd) = extract_msb3073_command(&msg) {
                msg = cmd;
            }
            if let Some(n) = exit_code {
                msg = format!("{} (exit code {})", msg, n);
            }
        }
        let severity = if kind.eq_ignore_ascii_case("warning") {
            Severity::Warning
        } else {
            Severity::Error
        };
        return Some(MsbuildDiag {
            idx,
            severity,
            code,
            file: Some(file),
            line: Some(lnum),
            message: msg,
            project,
            raw: line.to_string(),
        });
    }

    if let Some(caps) = MSVC_LINKER_RE.captures(line) {
        let kind = caps.get(2)?.as_str();
        let code = caps.get(3)?.as_str().to_string();
        let msg = caps.get(4)?.as_str().to_string();
        let severity = if kind.eq_ignore_ascii_case("warning") {
            Severity::Warning
        } else {
            Severity::Error
        };
        return Some(MsbuildDiag {
            idx,
            severity,
            code,
            file: None,
            line: None,
            message: msg,
            project,
            raw: line.to_string(),
        });
    }

    if let Some(caps) = MSVC_LINK_TOOL_RE.captures(line) {
        let kind = caps.get(1)?.as_str();
        let code = caps.get(2)?.as_str().to_string();
        let msg = caps.get(3)?.as_str().to_string();
        let severity = if kind.eq_ignore_ascii_case("warning") {
            Severity::Warning
        } else {
            Severity::Error
        };
        return Some(MsbuildDiag {
            idx,
            severity,
            code,
            file: None,
            line: None,
            message: msg,
            project,
            raw: line.to_string(),
        });
    }

    if MSB3073_RE.is_match(line) {
        let code = MSB3073_RE
            .captures(line)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "MSB3073".to_string());

        let mut msg = line.to_string();
        let exit_code = EXIT_CODE_RE
            .captures(&msg)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());
        if let Some(cmd) = extract_msb3073_command(line) {
            msg = cmd;
        }
        if let Some(n) = exit_code {
            msg = format!("{} (exit code {})", msg, n);
        }

        return Some(MsbuildDiag {
            idx,
            severity: Severity::Error,
            code,
            file: None,
            line: None,
            message: msg.clone(),
            project,
            raw: line.to_string(),
        });
    }

    None
}

fn extract_msb3073_command(msg: &str) -> Option<String> {
    // Expected shape:
    //   The command "...." exited with code N.
    // Command body may contain escaped quotes: \"C:\path with spaces\"
    let start = msg.find("The command \"")? + "The command \"".len();
    let rest = &msg[start..];
    let mut out = String::new();
    let mut escape = false;
    for (i, ch) in rest.char_indices() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }

        if ch == '\\' {
            // Only treat backslash as an escape marker when it escapes a quote or a backslash.
            // Otherwise it's a real Windows path separator.
            let next = rest[i + ch.len_utf8()..].chars().next();
            if matches!(next, Some('"') | Some('\\')) {
                escape = true;
            } else {
                out.push(ch);
            }
            continue;
        }

        if ch == '"' {
            let after = &rest[i + ch.len_utf8()..];
            if after.starts_with(" exited with code") {
                break;
            }
            out.push(ch);
            continue;
        }

        out.push(ch);
    }
    if out.is_empty() {
        return None;
    }

    // Normalize MSBuild escaping so paths are readable.
    // Keep this minimal: this is display output only.
    let out = out.replace(r#"\""#, r#"""#);
    Some(out)
}

fn first_real_error(diags: &[MsbuildDiag]) -> Option<MsbuildDiag> {
    diags.iter().find(|d| d.severity == Severity::Error).cloned()
}

fn dedup_diags(diags: &[MsbuildDiag]) -> Vec<MsbuildDiag> {
    let mut out: Vec<MsbuildDiag> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for d in diags {
        let key = format!("{}|{}", d.code, d.raw);
        if seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        out.push(d.clone());
    }
    out
}

struct ContextWindow {
    prev: Vec<String>,
    next: Vec<String>,
}

fn extract_context(lines: &[&str], idx: usize, prev_n: usize, next_n: usize) -> ContextWindow {
    let mut prev = Vec::new();
    let mut next = Vec::new();

    let mut i = idx;
    while i > 0 && prev.len() < prev_n {
        i -= 1;
        let t = lines[i].trim_end();
        if t.is_empty() {
            continue;
        }
        if is_msbuild_context_noise(t) {
            continue;
        }
        prev.push(sanitize_context_line(t));
    }
    prev.reverse();

    let mut j = idx + 1;
    while j < lines.len() && next.len() < next_n {
        let t = lines[j].trim_end();
        j += 1;
        if t.is_empty() {
            continue;
        }
        if is_msbuild_context_noise(t) {
            continue;
        }
        next.push(sanitize_context_line(t));
    }

    ContextWindow { prev, next }
}

fn is_msbuild_context_noise(line: &str) -> bool {
    let l = line.trim_start();
    let lower = l.to_ascii_lowercase();
    lower.starts_with("project \"")
        || lower.starts_with("done building project ")
        || lower.starts_with("build started ")
        || lower.starts_with("time elapsed ")
        || lower == "build failed."
        || lower == "build succeeded."
}

fn sanitize_context_line(line: &str) -> String {
    // Common MSBuild suffix noise: " ... [C:\path\Project.vcxproj]"
    // Keep behavior consistent with MSVC_COMPILER_RE stripping.
    if line.ends_with(']') && line.contains(".vcxproj") {
        if let Some(i) = line.rfind(" [") {
            return line[..i].to_string();
        }
    }
    line.to_string()
}

fn configuration_summary(args: &[String]) -> String {
    let solution = args
        .iter()
        .find(|a| {
            !a.starts_with('/')
                && !a.starts_with('-')
                && (a.ends_with(".sln")
                    || a.ends_with(".csproj")
                    || a.ends_with(".vcxproj")
                    || a.ends_with(".proj"))
        })
        .cloned()
        .unwrap_or_default();

    let mut config = String::new();
    let mut platform = String::new();
    for a in args {
        if let Some(rest) = a
            .strip_prefix("/p:Configuration=")
            .or_else(|| a.strip_prefix("-p:Configuration="))
        {
            config = rest.to_string();
        } else if let Some(rest) = a
            .strip_prefix("/p:Platform=")
            .or_else(|| a.strip_prefix("-p:Platform="))
        {
            platform = rest.to_string();
        }
    }

    let mut parts = Vec::new();
    if !solution.is_empty() {
        parts.push(solution);
    }
    if !config.is_empty() && !platform.is_empty() {
        parts.push(format!("{}|{}", config, platform));
    } else if !config.is_empty() {
        parts.push(config);
    }
    parts.join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compiler_error_strips_project_suffix() {
        let raw = "C:\\src\\main.cpp(42): error C2065: 'foo': undeclared identifier [C:\\proj\\MyProject.vcxproj]\n\
                   Build FAILED.\n\
                       1 Error(s)\n";
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("main.cpp(42): error C2065"));
        assert!(!out.contains("[C:\\proj\\MyProject.vcxproj]"));
        assert!(out.contains("Build FAILED"));
    }

    #[test]
    fn test_linker_error_kept_verbatim() {
        let raw = "MyProject.lib(module.obj) : error LNK2001: unresolved external symbol \"void __cdecl foo()\"\n\
                   MyOtherDLL.dll : fatal error LNK1120: 3 unresolved externals\n\
                   Build FAILED.\n";
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("LNK2001"));
        assert!(out.contains("LNK1120"));
        assert!(out.contains("MyProject.lib(module.obj)"));
    }

    #[test]
    fn test_success_compact() {
        let raw = "Microsoft (R) Build Engine version 17.8\n\
            Copyright (C) Microsoft Corporation.\n\
            \n\
            Build started 1/1/2025 12:00:00 PM.\n\
            Project \"MyProject.sln\" on node 1 (Build target(s)).\n\
              Copying file from x to y\n\
              Creating directory \"obj\\Debug\"\n\
              cl.exe /c main.cpp\n\
            Done Building Project \"MyProject.vcxproj\" (default targets).\n\
            \n\
            Build succeeded.\n\
                0 Warning(s)\n\
                0 Error(s)\n";
        let args = vec![
            "MyProject.sln".to_string(),
            "/p:Configuration=Debug".to_string(),
            "/p:Platform=Win32".to_string(),
        ];
        let out = filter_output(raw, &args);
        assert!(out.starts_with("msbuild: ok"));
        assert!(out.contains("MyProject.sln"));
        assert!(out.contains("Debug|Win32"));
    }

    #[test]
    fn test_empty_output() {
        let args = vec!["MyProject.sln".to_string(), "/t:Build".to_string()];
        let out = filter_output("", &args);
        assert!(out.contains("no output captured"));
    }

    #[test]
    fn test_fixture_success() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_success.txt");
        let args = vec![
            "MyProject.sln".to_string(),
            "/p:Configuration=Debug".to_string(),
            "/p:Platform=Win32".to_string(),
        ];
        let out = filter_output(raw, &args);
        assert!(out.starts_with("msbuild: ok"));
        assert!(out.contains("Debug|Win32"));
    }

    #[test]
    fn test_fixture_compiler_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_compiler.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("C2065"));
        assert!(out.contains("C2143"));
        assert!(out.contains("C1004"));
        assert!(!out.contains("[C:\\src\\MyProject\\MyProject.vcxproj]"));
    }

    #[test]
    fn test_fixture_linker_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_linker.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("LNK2001"));
        assert!(out.contains("LNK1120"));
        assert!(out.contains("MyProject.lib(util.obj)"));
    }

    #[test]
    fn test_fixture_rc_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_rc.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("RC1015"));
        assert!(out.contains("FIRST_ERROR"));
        assert!(out.contains("FAILED_PROJECTS"));
    }

    #[test]
    fn test_fixture_msb3073_extraction() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_msb3073.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("MSB3073"));
        assert!(out.contains("exit code 1"));
        assert!(out.contains("copy /Y"));
        assert!(out.contains("C:\\path with spaces\\out.dll"));
        assert!(out.contains("C:\\dest\\bin"));
        assert!(out.contains("FIRST_ERROR"));
    }

    #[test]
    fn test_fixture_msb8012_detection() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_failure_msb8012.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("MSB8012"));
        assert!(out.contains("TargetPath"));
        assert!(out.contains("FIRST_ERROR"));
    }

    #[test]
    fn test_fixture_empty_link() {
        let raw = include_str!("../../../tests/fixtures/cpp/msbuild_empty_link.txt");
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.contains("no output captured"));
    }

    #[test]
    fn test_warnings_dropped_on_success() {
        let raw = "C:\\src\\main.cpp(43): warning C4244: conversion from 'double' to 'int' [C:\\proj\\MyProject.vcxproj]\n\
                   Build succeeded.\n\
                       1 Warning(s)\n\
                       0 Error(s)\n";
        let args = vec!["MyProject.sln".to_string()];
        let out = filter_output(raw, &args);
        assert!(out.starts_with("msbuild: ok"));
        assert!(!out.contains("C4244"));
    }
}

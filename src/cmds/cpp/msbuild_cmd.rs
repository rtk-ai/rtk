//! Filters MSBuild output — keeps cl/link diagnostics, drops project/task noise.
//!
//! Captures stdout AND stderr (default for run_filtered) so linker errors from
//! `link.exe` (which writes to stderr) survive into the filter input.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    // Compiler: file(line): error|warning C1234: message [project.vcxproj]
    static ref MSVC_COMPILER_RE: Regex =
        Regex::new(r"^(.+)\((\d+)\): (error|warning|fatal error) (C\d+): (.+?)(?:\s+\[.+\])?$")
            .unwrap();
    // Linker: module : error|fatal error LNK1234: message
    static ref MSVC_LINKER_RE: Regex =
        Regex::new(r"^(.+) : (error|fatal error) (LNK\d+): (.+)$").unwrap();
    // Build FAILED. or Build succeeded.
    static ref BUILD_RESULT_RE: Regex = Regex::new(r"^Build (FAILED|succeeded)\.").unwrap();
    // "    N Error(s)" or "    N Warning(s)"
    static ref ERR_WARN_COUNT_RE: Regex = Regex::new(r"^\s+\d+\s+(Error|Warning)\(s\)").unwrap();
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
    let mut compiler_errors: Vec<String> = Vec::new();
    let mut compiler_warnings: Vec<String> = Vec::new();
    let mut linker: Vec<String> = Vec::new();
    let mut summary: Vec<String> = Vec::new();
    let mut build_result: Option<String> = None;
    let mut succeeded = false;

    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(caps) = MSVC_COMPILER_RE.captures(trimmed) {
            let file = &caps[1];
            let lnum = &caps[2];
            let kind = &caps[3];
            let code = &caps[4];
            let msg = &caps[5];
            let formatted = format!("{}({}): {} {}: {}", file, lnum, kind, code, msg);
            if kind == "warning" {
                compiler_warnings.push(formatted);
            } else {
                compiler_errors.push(formatted);
            }
            continue;
        }
        if MSVC_LINKER_RE.is_match(trimmed) {
            linker.push(trimmed.to_string());
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

    let has_errors = !compiler_errors.is_empty() || !linker.is_empty();

    if !has_errors && build_result.is_none() && compiler_warnings.is_empty() {
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
    for c in &compiler_errors {
        out.push_str(c);
        out.push('\n');
    }
    // Show warnings only on failure
    if !succeeded {
        for c in &compiler_warnings {
            out.push_str(c);
            out.push('\n');
        }
    }
    for l in &linker {
        out.push_str(l);
        out.push('\n');
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

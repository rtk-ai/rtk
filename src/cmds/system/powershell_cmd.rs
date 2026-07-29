use crate::core::tracking::TimedExecution;
use crate::core::utils::{exit_code_from_status, resolved_command};
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PsToken {
    value: String,
    raw: String,
}

impl PsToken {
    fn as_rtk_arg(&self) -> RtkArg {
        RtkArg {
            value: self.value.clone(),
            display: self.raw.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RtkArg {
    value: String,
    display: String,
}

impl RtkArg {
    fn literal(value: &str) -> Self {
        Self {
            value: value.to_string(),
            display: value.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RtkRewrite {
    args: Vec<RtkArg>,
}

impl RtkRewrite {
    fn command_line(&self) -> String {
        format!(
            "rtk {}",
            self.args
                .iter()
                .map(|arg| arg.display.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
    }

    fn process_args(&self) -> impl Iterator<Item = &str> {
        self.args.iter().map(|arg| arg.value.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    Single,
    Double,
}

/// Return a hook-safe RTK replacement for a PowerShell executable or a
/// supported bare cmdlet. Unsupported PowerShell syntax returns `None`.
pub fn rewrite_for_hook(command: &str) -> Option<String> {
    rewrite_shell_wrapper(command).or_else(|| rewrite_cmdlet(command).map(|r| r.command_line()))
}

/// Execute `powershell`/`pwsh`, dispatching a safe `-Command` cmdlet through
/// RTK and preserving all other invocations as transparent passthrough.
pub fn run(shell: &str, args: &[String], verbose: u8) -> Result<i32> {
    if let Some(script) = extract_rewritable_script(args) {
        if let Some(rewrite) = rewrite_cmdlet(&script) {
            if verbose > 0 {
                eprintln!("PowerShell rewrite: {}", rewrite.command_line());
            }
            let current_exe =
                std::env::current_exe().context("Failed to locate the current rtk executable")?;
            let status = Command::new(current_exe)
                .args(rewrite.process_args())
                .status()
                .context("Failed to execute rewritten PowerShell command")?;
            return Ok(exit_code_from_status(&status, "PowerShell rewrite"));
        }
    }

    let timer = TimedExecution::start();
    let status = resolved_command(shell)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {shell}"))?;
    let raw = format_command(shell, args);
    timer.track_passthrough(&raw, &format!("rtk {raw} (passthrough)"));
    Ok(exit_code_from_status(&status, shell))
}

fn format_command(shell: &str, args: &[String]) -> String {
    if args.is_empty() {
        shell.to_string()
    } else {
        format!("{shell} {}", args.join(" "))
    }
}

fn extract_rewritable_script(args: &[String]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        match args[index].to_ascii_lowercase().as_str() {
            "-noprofile" | "-noninteractive" | "-nologo" => index += 1,
            "-command" | "-c" => {
                let script = args.get(index + 1..)?.join(" ");
                return (!script.trim().is_empty()).then_some(script);
            }
            _ => return None,
        }
    }
    None
}

fn rewrite_shell_wrapper(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let (head, head_end) = first_token(trimmed)?;
    let executable = head
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(head.as_str())
        .to_ascii_lowercase();
    let canonical = match executable.as_str() {
        "powershell" | "powershell.exe" => "powershell",
        "pwsh" | "pwsh.exe" => "pwsh",
        _ => return None,
    };
    Some(format!("rtk {canonical}{}", &trimmed[head_end..]))
}

fn first_token(input: &str) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut quote = None;
    let mut escaped = false;

    for (offset, ch) in input.char_indices() {
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        match quote {
            Some(Quote::Single) => {
                if ch == '\'' {
                    quote = None;
                } else {
                    value.push(ch);
                }
            }
            Some(Quote::Double) => match ch {
                '"' => quote = None,
                '`' => escaped = true,
                _ => value.push(ch),
            },
            None => match ch {
                '\'' => quote = Some(Quote::Single),
                '"' => quote = Some(Quote::Double),
                '`' => escaped = true,
                c if c.is_whitespace() => return (!value.is_empty()).then_some((value, offset)),
                _ => value.push(ch),
            },
        }
    }

    if quote.is_none() && !escaped && !value.is_empty() {
        Some((value, input.len()))
    } else {
        None
    }
}

fn rewrite_cmdlet(command: &str) -> Option<RtkRewrite> {
    let tokens = ps_split(command)?;
    let (head, args) = tokens.split_first()?;
    match head.value.to_ascii_lowercase().as_str() {
        "get-content" | "gc" | "type" => rewrite_get_content(args),
        "get-childitem" | "gci" | "dir" | "ls" => rewrite_get_child_item(args),
        "select-string" | "sls" => rewrite_select_string(args),
        _ => None,
    }
}

fn rewrite_get_content(tokens: &[PsToken]) -> Option<RtkRewrite> {
    let mut paths = Vec::new();
    let mut max_lines = None;
    let mut tail_lines = None;
    let mut index = 0;

    while index < tokens.len() {
        let lower = tokens[index].value.to_ascii_lowercase();
        match lower.as_str() {
            "-path" | "-literalpath" => {
                let path = tokens.get(index + 1)?;
                if has_wildcard(&path.value) {
                    return None;
                }
                paths.push(path.as_rtk_arg());
                index += 2;
            }
            "-totalcount" => {
                let count = tokens.get(index + 1)?;
                count.value.parse::<usize>().ok()?;
                max_lines = Some(count.as_rtk_arg());
                index += 2;
            }
            "-tail" => {
                let count = tokens.get(index + 1)?;
                count.value.parse::<usize>().ok()?;
                tail_lines = Some(count.as_rtk_arg());
                index += 2;
            }
            _ if lower.starts_with('-') => return None,
            _ => {
                if has_wildcard(&tokens[index].value) {
                    return None;
                }
                paths.push(tokens[index].as_rtk_arg());
                index += 1;
            }
        }
    }

    if paths.is_empty() || (max_lines.is_some() && tail_lines.is_some()) {
        return None;
    }

    let mut args = vec![RtkArg::literal("read")];
    args.extend(paths);
    if let Some(count) = max_lines {
        args.push(RtkArg::literal("--max-lines"));
        args.push(count);
    }
    if let Some(count) = tail_lines {
        args.push(RtkArg::literal("--tail-lines"));
        args.push(count);
    }
    Some(RtkRewrite { args })
}

fn rewrite_get_child_item(tokens: &[PsToken]) -> Option<RtkRewrite> {
    let mut paths = Vec::new();
    let mut recurse = false;
    let mut force = false;
    let mut depth = None;
    let mut index = 0;

    while index < tokens.len() {
        let lower = tokens[index].value.to_ascii_lowercase();
        match lower.as_str() {
            "-path" | "-literalpath" => {
                let path = tokens.get(index + 1)?;
                if has_wildcard(&path.value) {
                    return None;
                }
                paths.push(path.as_rtk_arg());
                index += 2;
            }
            "-recurse" => {
                recurse = true;
                index += 1;
            }
            "-force" => {
                force = true;
                index += 1;
            }
            "-depth" => {
                let value = tokens.get(index + 1)?;
                value.value.parse::<usize>().ok()?;
                depth = Some(value.as_rtk_arg());
                recurse = true;
                index += 2;
            }
            _ if lower.starts_with('-') => return None,
            _ => {
                if has_wildcard(&tokens[index].value) {
                    return None;
                }
                paths.push(tokens[index].as_rtk_arg());
                index += 1;
            }
        }
    }

    let mut args = vec![RtkArg::literal(if recurse { "tree" } else { "ls" })];
    if force {
        args.push(RtkArg::literal("-a"));
    }
    if let Some(value) = depth {
        args.push(RtkArg::literal("-L"));
        args.push(value);
    }
    args.extend(paths);
    Some(RtkRewrite { args })
}

fn rewrite_select_string(tokens: &[PsToken]) -> Option<RtkRewrite> {
    let mut pattern = None;
    let mut paths = Vec::new();
    let mut case_sensitive = false;
    let mut invert_match = false;
    let mut index = 0;

    while index < tokens.len() {
        let lower = tokens[index].value.to_ascii_lowercase();
        match lower.as_str() {
            "-pattern" => {
                let value = tokens.get(index + 1)?;
                pattern = Some(value.as_rtk_arg());
                index += 2;
            }
            "-path" | "-literalpath" => {
                let value = tokens.get(index + 1)?;
                if has_wildcard(&value.value) {
                    return None;
                }
                paths.push(value.as_rtk_arg());
                index += 2;
            }
            "-casesensitive" => {
                case_sensitive = true;
                index += 1;
            }
            "-notmatch" => {
                invert_match = true;
                index += 1;
            }
            _ if lower.starts_with('-') => return None,
            _ if pattern.is_none() => {
                pattern = Some(tokens[index].as_rtk_arg());
                index += 1;
            }
            _ => {
                if has_wildcard(&tokens[index].value) {
                    return None;
                }
                paths.push(tokens[index].as_rtk_arg());
                index += 1;
            }
        }
    }

    let pattern = pattern?;
    if paths.is_empty() {
        return None;
    }

    let mut args = vec![RtkArg::literal("grep")];
    if !case_sensitive {
        args.push(RtkArg::literal("-i"));
    }
    if invert_match {
        args.push(RtkArg::literal("-v"));
    }
    args.push(pattern);
    args.extend(paths);
    Some(RtkRewrite { args })
}

fn has_wildcard(value: &str) -> bool {
    value.contains(['*', '?'])
}

/// PowerShell-aware tokenization for the conservative cmdlet subset above.
/// Backslashes remain literal; backticks escape the next character; compound
/// syntax and dynamic interpolation are rejected.
fn ps_split(input: &str) -> Option<Vec<PsToken>> {
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut tokens = Vec::new();
    let mut token_start = None;
    let mut value = String::new();
    let mut quote = None;
    let mut index = 0;

    while index < chars.len() {
        let (offset, ch) = chars[index];
        match quote {
            Some(Quote::Single) => {
                if ch == '\'' {
                    if chars.get(index + 1).is_some_and(|(_, next)| *next == '\'') {
                        value.push('\'');
                        index += 2;
                        continue;
                    }
                    quote = None;
                } else {
                    value.push(ch);
                }
            }
            Some(Quote::Double) => match ch {
                '"' => quote = None,
                '`' => {
                    let (_, escaped) = chars.get(index + 1)?;
                    value.push(*escaped);
                    index += 2;
                    continue;
                }
                '$' => return None,
                _ => value.push(ch),
            },
            None => match ch {
                c if c.is_whitespace() => {
                    if let Some(start) = token_start.take() {
                        tokens.push(PsToken {
                            value: std::mem::take(&mut value),
                            raw: input[start..offset].to_string(),
                        });
                    }
                }
                '\'' => {
                    token_start.get_or_insert(offset);
                    quote = Some(Quote::Single);
                }
                '"' => {
                    token_start.get_or_insert(offset);
                    quote = Some(Quote::Double);
                }
                '`' => {
                    token_start.get_or_insert(offset);
                    let (_, escaped) = chars.get(index + 1)?;
                    value.push(*escaped);
                    index += 2;
                    continue;
                }
                '|' | ';' | '&' | '<' | '>' | '{' | '}' | '(' | ')' | ',' | '$' => return None,
                '#' if token_start.is_none() => return None,
                _ => {
                    token_start.get_or_insert(offset);
                    value.push(ch);
                }
            },
        }
        index += 1;
    }

    if quote.is_some() {
        return None;
    }
    if let Some(start) = token_start {
        tokens.push(PsToken {
            value,
            raw: input[start..].to_string(),
        });
    }
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powershell_split_preserves_windows_paths_and_raw_quotes() {
        let tokens = ps_split(r#"Select-String -Pattern "fn run" -Path src\main.rs"#).unwrap();
        assert_eq!(tokens[2].value, "fn run");
        assert_eq!(tokens[2].raw, r#""fn run""#);
        assert_eq!(tokens[4].value, r"src\main.rs");
    }

    #[test]
    fn powershell_split_rejects_dynamic_or_compound_scripts() {
        for command in [
            "Get-ChildItem | Where-Object Length",
            "Get-Content $env:TEMP",
            "Get-Content a; Remove-Item a",
            "Get-Content (Join-Path a b)",
        ] {
            assert!(ps_split(command).is_none(), "{command}");
        }
    }

    #[test]
    fn hook_rewrite_preserves_wrapper_arguments() {
        assert_eq!(
            rewrite_for_hook(r#"PowerShell.exe -NoProfile -Command "Get-ChildItem src""#).as_deref(),
            Some(r#"rtk powershell -NoProfile -Command "Get-ChildItem src""#)
        );
        assert_eq!(
            rewrite_for_hook(r#""C:\Program Files\PowerShell\7\pwsh.exe" -File script.ps1"#)
                .as_deref(),
            Some("rtk pwsh -File script.ps1")
        );
    }

    #[test]
    fn extracts_only_safe_command_invocations() {
        assert_eq!(
            extract_rewritable_script(&[
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "Get-ChildItem src".to_string(),
            ])
            .as_deref(),
            Some("Get-ChildItem src")
        );
        assert!(extract_rewritable_script(&[
            "-WorkingDirectory".to_string(),
            "C:\\".to_string(),
            "-Command".to_string(),
            "Get-ChildItem".to_string(),
        ])
        .is_none());
        assert!(extract_rewritable_script(&[
            "-EncodedCommand".to_string(),
            "RwBlAHQALQBEAGEAdABlAA==".to_string(),
        ])
        .is_none());
    }

    #[test]
    fn cmdlet_rewrites_cover_content_listing_and_search() {
        assert_eq!(
            rewrite_for_hook("Get-Content README.md -Tail 20").as_deref(),
            Some("rtk read README.md --tail-lines 20")
        );
        assert_eq!(
            rewrite_for_hook("Get-ChildItem -Depth 2 -Force src").as_deref(),
            Some("rtk tree -a -L 2 src")
        );
        assert_eq!(
            rewrite_for_hook(r#"Select-String -Pattern "fn run" -Path src\main.rs"#).as_deref(),
            Some(r#"rtk grep -i "fn run" src\main.rs"#)
        );
    }

    #[test]
    fn unsupported_cmdlet_options_do_not_rewrite() {
        for command in [
            "Get-Content -Raw README.md",
            "Get-ChildItem -Filter *.rs",
            "Select-String -Context 2,2 TODO src",
        ] {
            assert_eq!(rewrite_for_hook(command), None, "{command}");
        }
    }
}

use std::ffi::OsString;

use super::powershell_lexer::{parse_static_argv, render_static_argv};

#[derive(Debug, PartialEq, Eq)]
pub struct GetContentSpec {
    pub file: OsString,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SelectStringSpec {
    pub pattern: String,
    pub path: String,
    pub ignore_case: bool,
    pub simple_match: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GetChildItemSpec {
    pub path: Option<String>,
    pub force: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct GetCommandSpec {
    pub name: String,
}

pub fn rewrite_which_raw(raw: &str) -> Option<String> {
    let argv = parse_static_argv(raw)?;
    let (cmd, rest) = argv.split_first()?;
    if !cmd.eq_ignore_ascii_case("which") || rest.len() != 1 {
        return None;
    }
    let name = rest[0].clone();
    if is_invalid_command_name(&name) {
        return None;
    }
    Some(render_static_argv(&[
        "rtk".to_string(),
        "which".to_string(),
        name,
    ]))
}

pub fn rewrite_get_command_raw(raw: &str) -> Option<String> {
    let argv = parse_static_argv(raw)?;
    let spec = parse_get_command_strings(&argv)?;
    Some(render_static_argv(&[
        "rtk".to_string(),
        "which".to_string(),
        spec.name,
    ]))
}

pub fn rewrite_get_content_raw(raw: &str) -> Option<String> {
    let argv = parse_static_argv(raw)?;
    let spec = parse_get_content_strings(&argv, false)?;
    let file = spec.file.into_string().ok()?;
    Some(render_static_argv(&[
        "rtk".to_string(),
        "read".to_string(),
        file,
    ]))
}

pub fn rewrite_select_string_raw(raw: &str) -> Option<String> {
    let argv = parse_static_argv(raw)?;
    let spec = parse_select_string_strings(&argv)?;
    Some(render_select_string_rewrite(&spec))
}

pub fn rewrite_get_child_item_raw(raw: &str) -> Option<String> {
    let argv = parse_static_argv(raw)?;
    let spec = parse_get_child_item_strings(&argv)?;
    let mut output = vec!["rtk".to_string(), "ls".to_string()];
    if spec.force {
        output.push("-a".to_string());
    }
    if let Some(path) = spec.path {
        output.push(path);
    }
    Some(render_static_argv(&output))
}

pub fn parse_get_content_argv(args: &[OsString]) -> Option<GetContentSpec> {
    let argv = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    parse_get_content_strings(&argv, true)
}

pub fn parse_select_string_argv(args: &[OsString]) -> Option<SelectStringSpec> {
    let argv = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    parse_select_string_strings(&argv)
}

pub fn parse_get_child_item_argv(args: &[OsString]) -> Option<GetChildItemSpec> {
    let argv = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    parse_get_child_item_strings(&argv)
}

pub fn parse_get_command_argv(args: &[OsString]) -> Option<GetCommandSpec> {
    let argv = args
        .iter()
        .map(|arg| arg.to_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()?;
    parse_get_command_strings(&argv)
}

fn render_select_string_rewrite(spec: &SelectStringSpec) -> String {
    let mut argv = vec!["rtk".to_string(), "grep".to_string()];
    if spec.ignore_case {
        argv.push("-i".to_string());
    }
    let pattern = if spec.simple_match {
        regex::escape(&spec.pattern)
    } else {
        spec.pattern.clone()
    };
    argv.push(pattern);
    argv.push(spec.path.clone());
    render_static_argv(&argv)
}

fn parse_get_child_item_strings(args: &[String]) -> Option<GetChildItemSpec> {
    let (cmd, rest) = args.split_first()?;
    if !cmd.eq_ignore_ascii_case("Get-ChildItem") {
        return None;
    }

    let mut path: Option<String> = None;
    let mut force = false;
    let mut i = 0;
    while i < rest.len() {
        let token = &rest[i];
        if token.starts_with('-') {
            if token.eq_ignore_ascii_case("-Force") {
                force = true;
                i += 1;
                continue;
            }
            if token.eq_ignore_ascii_case("-LiteralPath") {
                path = set_once(path, rest.get(i + 1)?.clone())?;
                if is_dynamic_or_provider_path(path.as_ref()?) {
                    return None;
                }
                i += 2;
                continue;
            }
            if token.eq_ignore_ascii_case("-Path") {
                let value = rest.get(i + 1)?.clone();
                if has_powershell_wildcard(&value) || is_dynamic_or_provider_path(&value) {
                    return None;
                }
                path = set_once(path, value)?;
                i += 2;
                continue;
            }
            return None;
        }

        if has_powershell_wildcard(token) || is_dynamic_or_provider_path(token) {
            return None;
        }
        path = set_once(path, token.clone())?;
        i += 1;
    }

    Some(GetChildItemSpec { path, force })
}

fn parse_get_command_strings(args: &[String]) -> Option<GetCommandSpec> {
    let (cmd, rest) = args.split_first()?;
    if !cmd.eq_ignore_ascii_case("Get-Command") {
        return None;
    }

    let mut command_type_application = false;
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        let token = &rest[i];
        if token.starts_with('-') {
            if token.eq_ignore_ascii_case("-CommandType") {
                let value = rest.get(i + 1)?;
                if !value.eq_ignore_ascii_case("Application") || command_type_application {
                    return None;
                }
                command_type_application = true;
                i += 2;
                continue;
            }
            if let Some(value) = token.strip_prefix("-CommandType:") {
                if !value.eq_ignore_ascii_case("Application") || command_type_application {
                    return None;
                }
                command_type_application = true;
                i += 1;
                continue;
            }
            if token.eq_ignore_ascii_case("-Name") {
                name = set_once(name, rest.get(i + 1)?.clone())?;
                i += 2;
                continue;
            }
            return None;
        }

        name = set_once(name, token.clone())?;
        i += 1;
    }

    if !command_type_application {
        return None;
    }

    let name = name?;
    if is_invalid_command_name(&name) {
        return None;
    }

    Some(GetCommandSpec { name })
}

fn parse_get_content_strings(args: &[String], allow_boundary: bool) -> Option<GetContentSpec> {
    let (cmd, rest) = args.split_first()?;
    if !cmd.eq_ignore_ascii_case("Get-Content") {
        return None;
    }

    let mut file: Option<String> = None;
    let mut literal_mode = false;
    let mut i = 0;
    while i < rest.len() {
        let token = &rest[i];
        if allow_boundary && !literal_mode && token == "--" {
            literal_mode = true;
            i += 1;
            continue;
        }

        if !literal_mode && token.starts_with('-') {
            if token.eq_ignore_ascii_case("-Encoding") {
                let value = rest.get(i + 1)?;
                if !is_supported_utf8_encoding(value) {
                    return None;
                }
                i += 2;
                continue;
            }
            if let Some(value) = token.strip_prefix("-Encoding:") {
                if !is_supported_utf8_encoding(value) {
                    return None;
                }
                i += 1;
                continue;
            }
            return None;
        }

        if file.is_some() || is_dynamic_or_provider_path(token) || token == "-" {
            return None;
        }
        file = Some(token.clone());
        i += 1;
    }

    Some(GetContentSpec {
        file: OsString::from(file?),
    })
}

fn parse_select_string_strings(args: &[String]) -> Option<SelectStringSpec> {
    let (cmd, rest) = args.split_first()?;
    if !cmd.eq_ignore_ascii_case("Select-String") {
        return None;
    }

    let mut pattern: Option<String> = None;
    let mut path: Option<String> = None;
    let mut ignore_case = true;
    let mut simple_match = false;

    let mut i = 0;
    while i < rest.len() {
        let token = &rest[i];
        if token.starts_with('-') {
            if token.eq_ignore_ascii_case("-Pattern") {
                pattern = set_once(pattern, rest.get(i + 1)?.clone())?;
                i += 2;
                continue;
            }
            if token.eq_ignore_ascii_case("-Path") || token.eq_ignore_ascii_case("-LiteralPath") {
                path = set_once(path, rest.get(i + 1)?.clone())?;
                i += 2;
                continue;
            }
            if token.eq_ignore_ascii_case("-CaseSensitive") {
                ignore_case = false;
                i += 1;
                continue;
            }
            if token.eq_ignore_ascii_case("-SimpleMatch") {
                simple_match = true;
                i += 1;
                continue;
            }
            return None;
        }

        if pattern.is_none() {
            pattern = Some(token.clone());
        } else if path.is_none() {
            path = Some(token.clone());
        } else {
            return None;
        }
        i += 1;
    }

    let pattern = pattern?;
    let path = path?;
    if is_dynamic_or_provider_path(&pattern) || is_dynamic_or_provider_path(&path) {
        return None;
    }

    Some(SelectStringSpec {
        pattern,
        path,
        ignore_case,
        simple_match,
    })
}

fn set_once<T>(slot: Option<T>, value: T) -> Option<Option<T>> {
    if slot.is_some() {
        None
    } else {
        Some(Some(value))
    }
}

fn is_supported_utf8_encoding(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "utf8" | "utf8bom" | "utf8nobom"
    )
}

fn is_dynamic_or_provider_path(value: &str) -> bool {
    if let Some(unc_path) = value.strip_prefix(r"\\") {
        let mut components = unc_path.split('\\');
        let server = components.next().unwrap_or_default();
        let share = components.next().unwrap_or_default();
        let static_share =
            !share.is_empty() && share.ends_with('$') && !share[..share.len() - 1].contains('$');

        return server.contains('$')
            || (share.contains('$') && !static_share)
            || components.any(|component| component.contains('$'));
    }

    if value.contains('$')
        || value.contains('`')
        || value.contains("$(")
        || value.contains('{')
        || value.contains('}')
        || value.contains(';')
    {
        return true;
    }
    if let Some((prefix, _)) = value.split_once(":\\") {
        return !(prefix.len() == 1 && prefix.chars().all(|c| c.is_ascii_alphabetic()));
    }
    false
}

fn has_powershell_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[') || value.contains(']')
}

fn is_invalid_command_name(value: &str) -> bool {
    value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || has_powershell_wildcard(value)
        || is_dynamic_or_provider_path(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_get_content_basic() {
        assert_eq!(
            rewrite_get_content_raw("Get-Content foo.txt"),
            Some("rtk read foo.txt".to_string())
        );
    }

    #[test]
    fn rewrite_get_content_encoding_prefix() {
        assert_eq!(
            rewrite_get_content_raw("Get-Content -Encoding utf8 foo.txt"),
            Some("rtk read foo.txt".to_string())
        );
    }

    #[test]
    fn rewrite_get_content_encoding_suffix() {
        assert_eq!(
            rewrite_get_content_raw("Get-Content foo.txt -Encoding utf8NoBOM"),
            Some("rtk read foo.txt".to_string())
        );
    }

    #[test]
    fn rewrite_get_content_raw_passthrough() {
        assert_eq!(rewrite_get_content_raw("Get-Content -Raw foo.txt"), None);
    }

    #[test]
    fn rewrite_get_content_dynamic_path_passthrough() {
        assert_eq!(
            rewrite_get_content_raw("Get-Content $env:TEMP\\a.txt"),
            None
        );
    }

    #[test]
    fn rewrite_get_content_raw_unc_admin_and_dollar_shares() {
        assert_eq!(
            rewrite_get_content_raw(r"Get-Content \\server\c$\x.txt"),
            Some(r"rtk read \\server\c$\x.txt".to_string())
        );
        assert_eq!(
            rewrite_get_content_raw(r"Get-Content \\server\share$\x.txt"),
            Some(r"rtk read \\server\share$\x.txt".to_string())
        );
    }

    #[test]
    fn rewrite_get_content_stop_parsing_passthrough() {
        assert_eq!(rewrite_get_content_raw("Get-Content --% literal.txt"), None);
    }

    #[test]
    fn rewrite_get_content_variable_unc_share_passthrough() {
        assert_eq!(
            rewrite_get_content_raw(r"Get-Content \\server\$share\x.txt"),
            None
        );
    }

    #[test]
    fn rewrite_get_content_subexpression_passthrough() {
        assert_eq!(
            rewrite_get_content_raw("Get-Content $(Join-Path $env:TEMP x.txt)"),
            None
        );
    }

    #[test]
    fn rewrite_get_content_embedded_variable_passthrough() {
        assert_eq!(rewrite_get_content_raw("Get-Content file$number.txt"), None);
    }

    #[test]
    fn unc_admin_and_dollar_share_paths_are_static() {
        assert!(!is_dynamic_or_provider_path(r"\\server\c$"));
        assert!(!is_dynamic_or_provider_path(r"\\server\share$\dir"));
    }

    #[test]
    fn unc_path_with_variable_share_is_dynamic() {
        assert!(is_dynamic_or_provider_path(r"\\server\$share"));
        assert!(is_dynamic_or_provider_path(r"$env:TEMP\a.txt"));
    }

    #[test]
    fn rewrite_get_content_dash_is_not_stdin() {
        assert_eq!(rewrite_get_content_raw("Get-Content -"), None);
    }

    #[test]
    fn parse_get_content_argv_accepts_dash_literal_after_boundary() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("--"),
            OsString::from("-Raw"),
        ];
        assert_eq!(
            parse_get_content_argv(&args),
            Some(GetContentSpec {
                file: OsString::from("-Raw")
            })
        );
    }

    #[test]
    fn rewrite_select_string_named() {
        assert_eq!(
            rewrite_select_string_raw("Select-String -Pattern NEEDLE -Path src/a.rs"),
            Some("rtk grep -i NEEDLE src/a.rs".to_string())
        );
    }

    #[test]
    fn rewrite_select_string_positional() {
        assert_eq!(
            rewrite_select_string_raw("Select-String NEEDLE src/a.rs"),
            Some("rtk grep -i NEEDLE src/a.rs".to_string())
        );
    }

    #[test]
    fn rewrite_select_string_case_sensitive() {
        assert_eq!(
            rewrite_select_string_raw(
                "Select-String -CaseSensitive -Pattern NEEDLE -Path src/a.rs"
            ),
            Some("rtk grep NEEDLE src/a.rs".to_string())
        );
    }

    #[test]
    fn rewrite_select_string_simple_match_escapes_regex() {
        assert_eq!(
            rewrite_select_string_raw("Select-String -SimpleMatch -Pattern a.b -Path src/a.rs"),
            Some(r"rtk grep -i a\.b src/a.rs".to_string())
        );
    }

    #[test]
    fn rewrite_select_string_context_passthrough() {
        assert_eq!(
            rewrite_select_string_raw("Select-String -Context 2 NEEDLE src/a.rs"),
            None
        );
    }

    #[test]
    fn rewrite_get_child_item_empty() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem"),
            Some("rtk ls".to_string())
        );
    }

    #[test]
    fn rewrite_get_child_item_path() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem src"),
            Some("rtk ls src".to_string())
        );
    }

    #[test]
    fn rewrite_get_child_item_named_path() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem -Path src"),
            Some("rtk ls src".to_string())
        );
    }

    #[test]
    fn rewrite_get_child_item_literal_path() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem -LiteralPath src"),
            Some("rtk ls src".to_string())
        );
    }

    #[test]
    fn rewrite_get_child_item_force() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem -Force src"),
            Some("rtk ls -a src".to_string())
        );
    }

    #[test]
    fn rewrite_get_child_item_wildcard_path_passthrough() {
        assert_eq!(rewrite_get_child_item_raw("Get-ChildItem -Path *.rs"), None);
    }

    #[test]
    fn rewrite_get_child_item_recurse_filter_passthrough() {
        assert_eq!(
            rewrite_get_child_item_raw("Get-ChildItem -Recurse -Filter *.rs src"),
            None
        );
    }

    #[test]
    fn rewrite_get_child_item_name_passthrough() {
        assert_eq!(rewrite_get_child_item_raw("Get-ChildItem -Name src"), None);
    }

    #[test]
    fn rewrite_which() {
        assert_eq!(
            rewrite_which_raw("which cargo"),
            Some("rtk which cargo".to_string())
        );
    }

    #[test]
    fn rewrite_which_path_passthrough() {
        assert_eq!(rewrite_which_raw("which ./cargo"), None);
    }

    #[test]
    fn rewrite_where_exe_passthrough() {
        assert_eq!(rewrite_which_raw("where.exe cargo"), None);
    }

    #[test]
    fn rewrite_get_command_application() {
        assert_eq!(
            rewrite_get_command_raw("Get-Command -CommandType Application cargo"),
            Some("rtk which cargo".to_string())
        );
    }

    #[test]
    fn rewrite_get_command_application_named() {
        assert_eq!(
            rewrite_get_command_raw("Get-Command -CommandType Application -Name cargo"),
            Some("rtk which cargo".to_string())
        );
    }

    #[test]
    fn rewrite_get_command_bare_passthrough() {
        assert_eq!(rewrite_get_command_raw("Get-Command cargo"), None);
    }

    #[test]
    fn rewrite_get_command_alias_candidate_passthrough() {
        assert_eq!(rewrite_get_command_raw("Get-Command ls"), None);
    }

    #[test]
    fn rewrite_get_command_module_passthrough() {
        assert_eq!(
            rewrite_get_command_raw("Get-Command -Module Microsoft.PowerShell.Management"),
            None
        );
    }

    #[test]
    fn rewrite_get_command_syntax_passthrough() {
        assert_eq!(rewrite_get_command_raw("Get-Command -Syntax cargo"), None);
    }
}

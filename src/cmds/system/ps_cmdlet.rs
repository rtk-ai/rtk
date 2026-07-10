use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::Result;

use crate::core::filter::FilterLevel;
use crate::discover::ps_classify;

use super::{ls, read, search, which};

#[derive(Debug, PartialEq, Eq)]
pub enum CompatDirectResult {
    Handled(i32),
    Unsupported,
}

pub fn run_direct(args: &[OsString], verbose: u8) -> Result<CompatDirectResult> {
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        return Ok(CompatDirectResult::Unsupported);
    };

    if command.eq_ignore_ascii_case("Get-Content") {
        return run_get_content(args, verbose);
    }
    if command.eq_ignore_ascii_case("Select-String") {
        return run_select_string(args, verbose);
    }
    if command.eq_ignore_ascii_case("Get-ChildItem") {
        return run_get_child_item(args, verbose);
    }
    if command.eq_ignore_ascii_case("Get-Command") {
        return run_get_command(args);
    }

    Ok(CompatDirectResult::Unsupported)
}

fn run_get_content(args: &[OsString], verbose: u8) -> Result<CompatDirectResult> {
    let Some(spec) = ps_classify::parse_get_content_argv(args) else {
        return Ok(CompatDirectResult::Unsupported);
    };

    let path = PathBuf::from(spec.file);
    read::run(&path, FilterLevel::None, None, None, false, verbose)?;
    Ok(CompatDirectResult::Handled(0))
}

fn run_select_string(args: &[OsString], verbose: u8) -> Result<CompatDirectResult> {
    let Some(spec) = ps_classify::parse_select_string_argv(args) else {
        return Ok(CompatDirectResult::Unsupported);
    };

    let mut grep_args = Vec::new();
    if spec.ignore_case {
        grep_args.push("-i".to_string());
    }
    let pattern = if spec.simple_match {
        regex::escape(&spec.pattern)
    } else {
        spec.pattern
    };
    grep_args.push(pattern);
    grep_args.push(spec.path);

    let code = search::run(search::Engine::Grep, 80, 200, false, &grep_args, verbose)?;
    Ok(CompatDirectResult::Handled(code))
}

fn run_get_child_item(args: &[OsString], verbose: u8) -> Result<CompatDirectResult> {
    let Some(spec) = ps_classify::parse_get_child_item_argv(args) else {
        return Ok(CompatDirectResult::Unsupported);
    };

    let mut ls_args = Vec::new();
    if spec.force {
        ls_args.push("-a".to_string());
    }
    if let Some(path) = spec.path {
        ls_args.push(path);
    }

    let code = ls::run(&ls_args, verbose)?;
    Ok(CompatDirectResult::Handled(code))
}

fn run_get_command(args: &[OsString]) -> Result<CompatDirectResult> {
    let Some(spec) = ps_classify::parse_get_command_argv(args) else {
        return Ok(CompatDirectResult::Unsupported);
    };

    let code = which::run(&spec.name)?;
    Ok(CompatDirectResult::Handled(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_get_content_unsupported_falls_back() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("-Raw"),
            OsString::from("Cargo.toml"),
        ];
        assert_eq!(
            run_direct(&args, 0).unwrap(),
            CompatDirectResult::Unsupported
        );
    }

    #[test]
    fn non_get_content_is_unsupported() {
        let args = vec![OsString::from("Select-String"), OsString::from("needle")];
        assert_eq!(
            run_direct(&args, 0).unwrap(),
            CompatDirectResult::Unsupported
        );
    }

    #[test]
    fn direct_select_string_context_falls_back() {
        let args = vec![
            OsString::from("Select-String"),
            OsString::from("-Context"),
            OsString::from("2"),
            OsString::from("needle"),
            OsString::from("Cargo.toml"),
        ];
        assert_eq!(
            run_direct(&args, 0).unwrap(),
            CompatDirectResult::Unsupported
        );
    }

    #[test]
    fn direct_get_child_item_recurse_falls_back() {
        let args = vec![
            OsString::from("Get-ChildItem"),
            OsString::from("-Recurse"),
            OsString::from("src"),
        ];
        assert_eq!(
            run_direct(&args, 0).unwrap(),
            CompatDirectResult::Unsupported
        );
    }

    #[test]
    fn direct_get_command_application_handles_missing_name() {
        let args = vec![
            OsString::from("Get-Command"),
            OsString::from("-CommandType"),
            OsString::from("Application"),
            OsString::from("rtk-definitely-missing-command-for-test"),
        ];
        assert_eq!(run_direct(&args, 0).unwrap(), CompatDirectResult::Handled(1));
    }

    #[test]
    fn direct_get_command_bare_uses_transport() {
        let args = vec![OsString::from("Get-Command"), OsString::from("cargo")];
        assert_eq!(
            run_direct(&args, 0).unwrap(),
            CompatDirectResult::Unsupported
        );
    }
}

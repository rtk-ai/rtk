use base64::Engine;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

const MAX_ENCODED_SOURCE_BYTES: usize = 8 * 1024;
const MAX_ENCODED_COMMAND_UNITS: usize = 30_000;

#[derive(Debug)]
pub enum WindowsFallbackDecision {
    DirectShellHost {
        program: OsString,
        child_args: Vec<OsString>,
    },
    PowerShellTransport {
        host: OsString,
        mode: PowerShellTransportMode,
        child_args: Vec<OsString>,
    },
    DirectExternal {
        program: OsString,
        child_args: Vec<OsString>,
    },
    BatchTransport {
        program: OsString,
        child_args: Vec<OsString>,
    },
    RejectAmbiguous {
        message: String,
    },
}

#[derive(Debug)]
pub enum PowerShellTransportMode {
    EncodedCommand { script: String },
    File { path: PathBuf },
}

pub fn is_shell_host(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe" | "cmd" | "cmd.exe"
    )
}

pub fn encode_powershell(script: &str) -> String {
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn powershell_encoded_args(script: &str) -> Result<Vec<OsString>> {
    if script.len() > MAX_ENCODED_SOURCE_BYTES {
        anyhow::bail!(
            "PowerShell transport script is too large; write it to a .ps1 file and run powershell -File"
        );
    }
    let encoded = encode_powershell(script);
    let args = vec![
        OsString::from("-NoProfile"),
        OsString::from("-EncodedCommand"),
        OsString::from(encoded),
    ];
    let command_units = "powershell.exe ".encode_utf16().count()
        + args
            .iter()
            .map(|arg| arg.to_string_lossy().encode_utf16().count() + 1)
            .sum::<usize>();
    if command_units > MAX_ENCODED_COMMAND_UNITS {
        anyhow::bail!(
            "PowerShell encoded command line is too large; write it to a .ps1 file and run powershell -File"
        );
    }
    Ok(args)
}

fn resolve_os_path(command: &OsStr) -> Option<PathBuf> {
    let command = command.to_str()?;
    crate::core::utils::resolve_binary(command).ok()
}

fn resolve_powershell_host_path() -> Option<PathBuf> {
    crate::core::utils::resolve_binary("powershell.exe")
        .ok()
        .or_else(|| crate::core::utils::resolve_binary("pwsh.exe").ok())
}

fn path_or_original(resolved: Option<PathBuf>, original: &OsStr) -> OsString {
    resolved
        .map(|path| path.into_os_string())
        .unwrap_or_else(|| original.to_os_string())
}

fn extension_is(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn is_batch_path(path: &Path) -> bool {
    extension_is(path, "cmd") || extension_is(path, "bat")
}

fn reject(message: impl Into<String>) -> Result<WindowsFallbackDecision> {
    Ok(WindowsFallbackDecision::RejectAmbiguous {
        message: message.into(),
    })
}

pub fn decide(args: &[OsString]) -> Result<WindowsFallbackDecision> {
    decide_with(args, resolve_os_path, resolve_powershell_host_path)
}

pub fn decide_with<R, H>(
    args: &[OsString],
    resolve_program: R,
    resolve_powershell_host: H,
) -> Result<WindowsFallbackDecision>
where
    R: Fn(&OsStr) -> Option<PathBuf>,
    H: Fn() -> Option<PathBuf>,
{
    if args.is_empty() {
        return reject("Windows fallback transport requires a command");
    }

    let command = &args[0];
    let command_text = command.to_str();
    if command_text.map(is_shell_host).unwrap_or(false) {
        return Ok(WindowsFallbackDecision::DirectShellHost {
            program: path_or_original(resolve_program(command), command),
            child_args: args[1..].to_vec(),
        });
    }

    let original_path = Path::new(command);
    let resolved = resolve_program(command);
    let classified_path = resolved.as_deref().unwrap_or(original_path);

    if extension_is(classified_path, "ps1") {
        let Some(host) = resolve_powershell_host() else {
            return reject(
                "PowerShell host not found; use an explicit powershell or pwsh host when available",
            );
        };
        return Ok(WindowsFallbackDecision::PowerShellTransport {
            host: host.into_os_string(),
            mode: PowerShellTransportMode::File {
                path: classified_path.to_path_buf(),
            },
            child_args: args[1..].to_vec(),
        });
    }

    if is_batch_path(classified_path) {
        if let Err(err) = validate_batch_args(&args[1..]) {
            return reject(err.to_string());
        }
        return Ok(WindowsFallbackDecision::BatchTransport {
            program: path_or_original(resolved, command),
            child_args: args[1..].to_vec(),
        });
    }

    if let Some(path) = resolved {
        return Ok(WindowsFallbackDecision::DirectExternal {
            program: path.into_os_string(),
            child_args: args[1..].to_vec(),
        });
    }

    let Some(command_text) = command_text else {
        return reject(
            "unresolved non-Unicode command requires direct external execution or an explicit shell host",
        );
    };

    if is_known_cmdlet(command_text) {
        let script = match render_powershell_invocation(args) {
            Ok(script) => script,
            Err(err) => return reject(err.to_string()),
        };
        let Some(host) = resolve_powershell_host() else {
            return reject(
                "PowerShell host not found; use an explicit powershell or pwsh host when available",
            );
        };
        return Ok(WindowsFallbackDecision::PowerShellTransport {
            host: host.into_os_string(),
            mode: PowerShellTransportMode::EncodedCommand { script },
            child_args: Vec::new(),
        });
    }

    reject(
        "ambiguous Windows command; use `rtk powershell -NoProfile -Command ...`, `rtk run -c ...`, or an explicit executable path",
    )
}

pub fn is_known_cmdlet(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "get-content" | "select-string" | "get-childitem" | "get-command"
    )
}

pub fn quote_ps_literal(value: &OsStr) -> Result<String> {
    let value = value
        .to_str()
        .context("PowerShell transport requires valid Unicode arguments")?;
    Ok(format!("'{}'", value.replace('\'', "''")))
}

pub fn render_powershell_invocation(args: &[OsString]) -> Result<String> {
    if args.is_empty() {
        anyhow::bail!("PowerShell transport requires a command");
    }
    let cmd = args[0]
        .to_str()
        .context("PowerShell cmdlet name must be valid Unicode")?;
    if !is_known_cmdlet(cmd) {
        anyhow::bail!(
            "unsupported PowerShell cmdlet; use `rtk powershell -NoProfile -Command ...`"
        );
    }

    let mut rendered = vec![cmd.to_string()];
    let mut literal_mode = false;
    for arg in &args[1..] {
        let text = arg
            .to_str()
            .context("PowerShell transport requires valid Unicode arguments")?;
        if !literal_mode && text == "--" {
            literal_mode = true;
            continue;
        }
        if !literal_mode && looks_script_like(text) {
            anyhow::bail!(
                "ambiguous PowerShell syntax; use `rtk powershell -NoProfile -Command ...`"
            );
        }
        if !literal_mode && text.starts_with('-') && !looks_negative_number(text) {
            if is_transport_param(cmd, text) {
                rendered.push(text.to_string());
            } else {
                anyhow::bail!(
                    "ambiguous dash-prefixed argument; use `--` for a literal value or `rtk powershell -NoProfile -Command ...`"
                );
            }
        } else {
            rendered.push(quote_ps_literal(arg)?);
        }
    }

    Ok(rendered.join(" "))
}

fn looks_negative_number(text: &str) -> bool {
    let rest = text.strip_prefix('-').unwrap_or(text);
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

fn looks_script_like(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{')
        || trimmed.ends_with('}')
        || trimmed.contains("$_")
        || trimmed.contains("@'")
        || trimmed.contains("\"@")
        || trimmed.contains('\n')
        || trimmed.contains('\r')
}

fn is_transport_param(cmdlet: &str, token: &str) -> bool {
    let name = token
        .split_once(':')
        .map(|(name, _)| name)
        .unwrap_or(token)
        .to_ascii_lowercase();
    common_transport_params().contains(&name.as_str())
        || match cmdlet.to_ascii_lowercase().as_str() {
            "get-content" => [
                "-path",
                "-literalpath",
                "-encoding",
                "-delimiter",
                "-readcount",
                "-totalcount",
                "-tail",
                "-filter",
                "-include",
                "-exclude",
                "-stream",
                "-raw",
                "-wait",
                "-force",
                "-asbytestream",
            ]
            .contains(&name.as_str()),
            "select-string" => [
                "-pattern",
                "-path",
                "-literalpath",
                "-inputobject",
                "-encoding",
                "-context",
                "-include",
                "-exclude",
                "-culture",
                "-simplematch",
                "-casesensitive",
                "-quiet",
                "-list",
                "-notmatch",
                "-allmatches",
                "-raw",
                "-noemphasis",
            ]
            .contains(&name.as_str()),
            "get-childitem" => [
                "-path",
                "-literalpath",
                "-filter",
                "-include",
                "-exclude",
                "-depth",
                "-attributes",
                "-name",
                "-recurse",
                "-force",
                "-file",
                "-directory",
                "-hidden",
                "-readonly",
                "-system",
                "-followsymlink",
            ]
            .contains(&name.as_str()),
            "get-command" => [
                "-name",
                "-verb",
                "-noun",
                "-module",
                "-commandtype",
                "-parametername",
                "-parametertype",
                "-argumentlist",
                "-totalcount",
                "-all",
                "-listimported",
                "-syntax",
                "-showcommandinfo",
            ]
            .contains(&name.as_str()),
            _ => false,
        }
}

fn common_transport_params() -> &'static [&'static str] {
    &[
        "-erroraction",
        "-warningaction",
        "-informationaction",
        "-progressaction",
        "-errorvariable",
        "-warningvariable",
        "-informationvariable",
        "-outvariable",
        "-outbuffer",
        "-pipelinevariable",
        "-verbose",
        "-debug",
        "-whatif",
        "-confirm",
    ]
}

pub fn run_script(script: &str, _verbose: u8) -> Result<i32> {
    let args = match powershell_encoded_args(script) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("rtk: {}", err);
            return Ok(2);
        }
    };
    let Some(host) = resolve_powershell_host_path() else {
        eprintln!("rtk: PowerShell host not found; install powershell.exe or pwsh.exe");
        return Ok(2);
    };
    let status = Command::new(host)
        .args(args)
        .status()
        .with_context(|| "Failed to execute PowerShell script")?;
    Ok(crate::core::utils::exit_code_from_status(&status, "run -c"))
}

pub fn run_argv(args: &[OsString], verbose: u8) -> Result<i32> {
    run_other(args, verbose)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        return Ok(0);
    }
    let decision = decide(args)?;
    execute_decision(decision, verbose)
}

fn execute_decision(decision: WindowsFallbackDecision, verbose: u8) -> Result<i32> {
    match decision {
        WindowsFallbackDecision::RejectAmbiguous { message } => {
            eprintln!("rtk: {}", message);
            Ok(2)
        }
        WindowsFallbackDecision::DirectShellHost {
            program,
            child_args,
        } => {
            if verbose > 0 {
                eprintln!("Windows run shell host: {:?} {:?}", program, child_args);
            }
            spawn_and_wait(program, child_args)
        }
        WindowsFallbackDecision::DirectExternal {
            program,
            child_args,
        } => {
            if verbose > 0 {
                eprintln!("Windows direct external: {:?} {:?}", program, child_args);
            }
            spawn_and_wait(program, child_args)
        }
        WindowsFallbackDecision::BatchTransport {
            program,
            child_args,
        } => {
            if verbose > 0 {
                eprintln!("Windows batch transport: {:?} {:?}", program, child_args);
            }
            spawn_and_wait(program, child_args)
        }
        WindowsFallbackDecision::PowerShellTransport {
            host,
            mode,
            child_args,
        } => {
            let args = match mode {
                PowerShellTransportMode::EncodedCommand { script } => {
                    if verbose > 0 {
                        eprintln!("Windows PowerShell transport: {}", script);
                    }
                    match powershell_encoded_args(&script) {
                        Ok(args) => args,
                        Err(err) => {
                            eprintln!("rtk: {}", err);
                            return Ok(2);
                        }
                    }
                }
                PowerShellTransportMode::File { path } => powershell_file_args(&path, &child_args),
            };
            spawn_and_wait(host, args)
        }
    }
}

fn spawn_and_wait(program: OsString, child_args: Vec<OsString>) -> Result<i32> {
    let label = program.to_string_lossy().into_owned();
    let status = Command::new(&program)
        .args(&child_args)
        .status()
        .with_context(|| format!("Failed to execute: {}", label))?;
    Ok(crate::core::utils::exit_code_from_status(&status, &label))
}

pub fn powershell_file_args(path: &Path, args: &[OsString]) -> Vec<OsString> {
    let mut result = vec![
        OsString::from("-NoProfile"),
        OsString::from("-File"),
        path.as_os_str().to_os_string(),
    ];
    result.extend(args.iter().cloned());
    result
}

pub fn validate_batch_args(args: &[OsString]) -> Result<()> {
    const FORBIDDEN: &[char] = &[
        '"', '%', '!', '^', '&', '|', '<', '>', '(', ')', '\r', '\n', '\0',
    ];
    for arg in args {
        let text = arg
            .to_str()
            .context("cmd batch transport requires valid Unicode arguments")?;
        if text.chars().any(|c| FORBIDDEN.contains(&c)) {
            anyhow::bail!(
                "cmd batch transport cannot safely represent this argv; use `rtk cmd /d /s /c <script>`"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn resolved_path(path: &str) -> Option<PathBuf> {
        Some(PathBuf::from(path))
    }

    fn unresolved(_: &OsStr) -> Option<PathBuf> {
        None
    }

    #[test]
    fn detects_powershell_shell_host_case_insensitive() {
        assert!(is_shell_host("powershell"));
        assert!(is_shell_host("PowerShell.EXE"));
        assert!(is_shell_host("pwsh.exe"));
    }

    #[test]
    fn detects_cmd_shell_host_case_insensitive() {
        assert!(is_shell_host("cmd"));
        assert!(is_shell_host("CMD.EXE"));
    }

    #[test]
    fn encoded_command_uses_utf16le() {
        assert_eq!(encode_powershell("A"), "QQA=");
    }

    #[test]
    fn implicit_transport_never_bypasses_execution_policy() {
        let args = powershell_encoded_args("Write-Output 'x'").unwrap();
        let rendered: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        assert!(!rendered
            .iter()
            .any(|a| a.eq_ignore_ascii_case("-ExecutionPolicy")));
        assert!(rendered.iter().any(|a| a == "-EncodedCommand"));
    }

    #[test]
    fn encoded_source_over_limit_is_rejected() {
        let script = "x".repeat(8 * 1024 + 1);
        let err = powershell_encoded_args(&script).unwrap_err().to_string();
        assert!(err.contains(".ps1") || err.contains("-File"));
    }

    #[test]
    fn quotes_powershell_literal_with_spaces() {
        assert_eq!(
            quote_ps_literal(&OsString::from("hello world")).unwrap(),
            "'hello world'"
        );
    }

    #[test]
    fn quotes_powershell_literal_with_single_quote() {
        assert_eq!(
            quote_ps_literal(&OsString::from("can't")).unwrap(),
            "'can''t'"
        );
    }

    #[test]
    fn renders_parameter_names_bare() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("-LiteralPath"),
            OsString::from("a b.txt"),
        ];
        assert_eq!(
            render_powershell_invocation(&args).unwrap(),
            "Get-Content -LiteralPath 'a b.txt'"
        );
    }

    #[test]
    fn dash_literal_after_boundary_is_quoted() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("--"),
            OsString::from("-Raw"),
        ];
        assert_eq!(
            render_powershell_invocation(&args).unwrap(),
            "Get-Content '-Raw'"
        );
    }

    #[test]
    fn ambiguous_dash_literal_is_rejected() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("-DefinitelyNotAParam"),
        ];
        let err = render_powershell_invocation(&args).unwrap_err().to_string();
        assert!(err.contains("--") || err.contains("powershell"));
    }

    #[test]
    fn scriptblock_like_bare_args_are_rejected() {
        let args = vec![
            OsString::from("Get-ChildItem"),
            OsString::from("{ $_.Name }"),
        ];
        assert!(render_powershell_invocation(&args).is_err());
    }

    #[test]
    fn unknown_cmdlet_requires_explicit_host() {
        assert!(!is_known_cmdlet("Where-Object"));
    }

    #[test]
    fn batch_transport_accepts_safe_subset() {
        let args = vec![
            OsString::from(""),
            OsString::from("hello world"),
            OsString::from("can't"),
            OsString::from("路径\\file.txt"),
        ];
        assert!(validate_batch_args(&args).is_ok());
    }

    #[test]
    fn batch_transport_rejects_cmd_metacharacters() {
        for value in [
            "\"", "%PATH%", "!VAR!", "^", "&", "|", "<", ">", "(", ")", "\r", "\n",
        ] {
            let err = validate_batch_args(&[OsString::from(value)])
                .unwrap_err()
                .to_string();
            assert!(err.contains("cmd"));
        }
    }

    #[test]
    fn resolved_ps1_uses_powershell_file() {
        let args = powershell_file_args(Path::new("tool.ps1"), &[OsString::from("a b")]);
        let rendered: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
        assert_eq!(rendered, vec!["-NoProfile", "-File", "tool.ps1", "a b"]);
    }

    #[test]
    fn decision_routes_path_resolved_exe_to_direct_external() {
        let args = vec![OsString::from("tool"), OsString::from("a b")];
        let decision = decide_with(
            &args,
            |_| resolved_path("C:\\bin\\tool.exe"),
            || resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        );
        assert!(matches!(
            decision.unwrap(),
            WindowsFallbackDecision::DirectExternal { .. }
        ));
    }

    #[test]
    fn decision_routes_path_resolved_ps1_to_powershell_file() {
        let args = vec![OsString::from("tool"), OsString::from("a b")];
        let decision = decide_with(
            &args,
            |_| resolved_path("C:\\bin\\tool.ps1"),
            || resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        )
        .unwrap();
        match decision {
            WindowsFallbackDecision::PowerShellTransport {
                mode: PowerShellTransportMode::File { path },
                child_args,
                ..
            } => {
                assert_eq!(path, PathBuf::from("C:\\bin\\tool.ps1"));
                assert_eq!(child_args, vec![OsString::from("a b")]);
            }
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn decision_routes_path_resolved_batch_to_batch_transport() {
        let args = vec![OsString::from("tool"), OsString::from("safe arg")];
        let decision = decide_with(
            &args,
            |_| resolved_path("C:\\bin\\tool.cmd"),
            || resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        );
        assert!(matches!(
            decision.unwrap(),
            WindowsFallbackDecision::BatchTransport { .. }
        ));
    }

    #[test]
    fn decision_rejects_unsafe_path_resolved_batch_arg() {
        let args = vec![OsString::from("tool"), OsString::from("%PATH%")];
        let decision = decide_with(
            &args,
            |_| resolved_path("C:\\bin\\tool.bat"),
            || resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        );
        assert!(matches!(
            decision.unwrap(),
            WindowsFallbackDecision::RejectAmbiguous { .. }
        ));
    }

    #[test]
    fn decision_routes_known_unresolved_cmdlet_to_encoded_transport() {
        let args = vec![
            OsString::from("Get-Content"),
            OsString::from("-Raw"),
            OsString::from("Cargo.toml"),
        ];
        let decision = decide_with(&args, unresolved, || {
            resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
        })
        .unwrap();
        match decision {
            WindowsFallbackDecision::PowerShellTransport {
                mode: PowerShellTransportMode::EncodedCommand { script },
                ..
            } => assert_eq!(script, "Get-Content -Raw 'Cargo.toml'"),
            other => panic!("unexpected decision: {other:?}"),
        }
    }

    #[test]
    fn decision_rejects_unknown_unresolved_powershell_like_command() {
        let args = vec![
            OsString::from("Where-Object"),
            OsString::from("{ $_.Name }"),
        ];
        let decision = decide_with(&args, unresolved, || {
            resolved_path("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe")
        });
        assert!(matches!(
            decision.unwrap(),
            WindowsFallbackDecision::RejectAmbiguous { .. }
        ));
    }

    #[test]
    fn windows_execution_sinks_do_not_receive_joined_argv() {
        let source = include_str!("windows_shell.rs");
        assert!(!source.contains("args.join(\" \")"));
        assert!(!source.contains(".arg(\"-Command\")"));
        assert!(!source.contains(".arg(\"/c\")"));
    }
}

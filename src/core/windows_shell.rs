use base64::Engine;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PowerShellExecutionPolicy {
    BypassNeeded,
    BypassNotNeeded,
}

#[derive(Debug)]
pub struct PreparedPowerShellTransport {
    pub args: Vec<OsString>,
    pub temp_script: Option<tempfile::TempPath>,
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

fn powershell_encoded_args(host: &OsStr, script: &str) -> Result<Vec<OsString>> {
    if script.len() > MAX_ENCODED_SOURCE_BYTES {
        anyhow::bail!(
            "PowerShell transport script is too large; write it to a .ps1 file and run powershell -File"
        );
    }
    let args = encoded_command_args(script);
    let command_units = encoded_command_units(host, &args);
    if command_units > MAX_ENCODED_COMMAND_UNITS {
        anyhow::bail!(
            "PowerShell encoded command line is too large; write it to a .ps1 file and run powershell -File"
        );
    }
    Ok(args)
}

fn encoded_command_args(script: &str) -> Vec<OsString> {
    vec![
        OsString::from("-NoProfile"),
        OsString::from("-EncodedCommand"),
        OsString::from(encode_powershell(script)),
    ]
}

fn encoded_command_units(host: &OsStr, args: &[OsString]) -> usize {
    host.to_string_lossy().encode_utf16().count()
        + 1
        + args
            .iter()
            .map(|arg| arg.to_string_lossy().encode_utf16().count() + 1)
            .sum::<usize>()
}

pub fn prepare_powershell_transport(
    host: &OsStr,
    script: &str,
    policy: PowerShellExecutionPolicy,
) -> Result<PreparedPowerShellTransport> {
    if let Ok(args) = powershell_encoded_args(host, script) {
        return Ok(PreparedPowerShellTransport {
            args,
            temp_script: None,
        });
    }

    let mut file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .context("rtk: PowerShell file transport: create")?;
    file.write_all(&[0xEF, 0xBB, 0xBF])
        .context("rtk: PowerShell file transport: BOM")?;
    file.write_all(script.as_bytes())
        .context("rtk: PowerShell file transport: body")?;
    file.flush()
        .context("rtk: PowerShell file transport: flush")?;
    let temp_script = file.into_temp_path();
    let args = powershell_file_args(temp_script.as_ref(), &[], policy);
    Ok(PreparedPowerShellTransport {
        args,
        temp_script: Some(temp_script),
    })
}

fn prepare_automatic_powershell_transport(
    host: &OsStr,
    script: &str,
) -> Result<PreparedPowerShellTransport> {
    let policy = if powershell_encoded_args(host, script).is_err() {
        detect_execution_policy(host)
    } else {
        PowerShellExecutionPolicy::BypassNotNeeded
    };
    prepare_powershell_transport(host, script, policy)
}

fn classify_execution_policy(policy: &str) -> PowerShellExecutionPolicy {
    match policy.trim().to_ascii_lowercase().as_str() {
        "restricted" | "allsigned" => PowerShellExecutionPolicy::BypassNeeded,
        _ => PowerShellExecutionPolicy::BypassNotNeeded,
    }
}

fn terminate_execution_policy_probe(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn detect_execution_policy(host: &OsStr) -> PowerShellExecutionPolicy {
    let mut child = match Command::new(host)
        .args(["-NoProfile", "-Command", "Get-ExecutionPolicy"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return PowerShellExecutionPolicy::BypassNeeded,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => match child.wait_with_output() {
                Ok(output) => {
                    return classify_execution_policy(&String::from_utf8_lossy(&output.stdout));
                }
                Err(_) => return PowerShellExecutionPolicy::BypassNeeded,
            },
            Ok(Some(_)) => return PowerShellExecutionPolicy::BypassNeeded,
            Err(_) => {
                terminate_execution_policy_probe(&mut child);
                return PowerShellExecutionPolicy::BypassNeeded;
            }
            Ok(None) if Instant::now() >= deadline => {
                terminate_execution_policy_probe(&mut child);
                return PowerShellExecutionPolicy::BypassNeeded;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
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
    let Some(host) = resolve_powershell_host_path() else {
        eprintln!("rtk: PowerShell host not found; install powershell.exe or pwsh.exe");
        return Ok(2);
    };
    let prepared = match prepare_automatic_powershell_transport(host.as_os_str(), script) {
        Ok(prepared) => prepared,
        Err(err) => {
            eprintln!("{err:#}");
            return Ok(2);
        }
    };
    spawn_prepared_powershell(host.into_os_string(), prepared, "run -c")
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
                    let prepared = match prepare_automatic_powershell_transport(&host, &script) {
                        Ok(prepared) => prepared,
                        Err(err) => {
                            eprintln!("{err:#}");
                            return Ok(2);
                        }
                    };
                    return spawn_prepared_powershell(host, prepared, "PowerShell transport");
                }
                PowerShellTransportMode::File { path } => powershell_file_args(
                    &path,
                    &child_args,
                    PowerShellExecutionPolicy::BypassNotNeeded,
                ),
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

fn spawn_prepared_powershell(
    host: OsString,
    prepared: PreparedPowerShellTransport,
    label: &str,
) -> Result<i32> {
    let PreparedPowerShellTransport { args, temp_script } = prepared;
    let uses_file_transport = temp_script.is_some();
    let _temp_script = temp_script;
    let status = match Command::new(&host).args(&args).status() {
        Ok(status) => status,
        Err(err) if uses_file_transport => {
            eprintln!(
                "{}",
                format_powershell_file_transport_spawn_error(&host, &err)
            );
            return Ok(2);
        }
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "Failed to execute PowerShell {}: {}",
                    label,
                    host.to_string_lossy()
                )
            });
        }
    };
    Ok(crate::core::utils::exit_code_from_status(&status, label))
}

fn format_powershell_file_transport_spawn_error(host: &OsStr, err: &std::io::Error) -> String {
    format!(
        "rtk: PowerShell file transport: spawn {}: {}",
        host.to_string_lossy(),
        err
    )
}

pub fn powershell_file_args(
    path: &Path,
    args: &[OsString],
    policy: PowerShellExecutionPolicy,
) -> Vec<OsString> {
    let mut result = vec![OsString::from("-NoProfile")];
    if policy == PowerShellExecutionPolicy::BypassNeeded {
        result.push(OsString::from("-ExecutionPolicy"));
        result.push(OsString::from("Bypass"));
    }
    result.extend([OsString::from("-File"), path.as_os_str().to_os_string()]);
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
    fn short_implicit_transport_uses_encoded_command_without_policy_override() {
        let prepared = prepare_powershell_transport(
            OsStr::new("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
            "Write-Output 'x'",
            PowerShellExecutionPolicy::BypassNeeded,
        )
        .unwrap();
        let rendered: Vec<_> = prepared.args.iter().map(|a| a.to_string_lossy()).collect();
        assert!(prepared.temp_script.is_none());
        assert!(rendered.iter().any(|a| a == "-EncodedCommand"));
        assert!(!rendered
            .iter()
            .any(|a| a.eq_ignore_ascii_case("-ExecutionPolicy")));
    }

    #[test]
    fn oversized_implicit_transport_uses_file_with_bypass() {
        let prepared = prepare_powershell_transport(
            OsStr::new("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
            &"x".repeat(8 * 1024 + 1),
            PowerShellExecutionPolicy::BypassNeeded,
        )
        .unwrap();
        let rendered: Vec<_> = prepared.args.iter().map(|a| a.to_string_lossy()).collect();
        assert!(prepared.temp_script.is_some());
        assert!(rendered.iter().any(|a| a == "-File"));
        assert!(rendered.iter().any(|a| a == "-ExecutionPolicy"));
        assert!(!rendered.iter().any(|a| a == "-EncodedCommand"));
    }

    #[test]
    fn encoded_transport_estimate_uses_actual_host_path() {
        let host = OsString::from(format!("C:\\{}\\powershell.exe", "x".repeat(30_000)));
        let prepared = prepare_powershell_transport(
            &host,
            "Write-Output 'x'",
            PowerShellExecutionPolicy::BypassNeeded,
        )
        .unwrap();
        assert!(prepared.temp_script.is_some());
        assert!(prepared.args.iter().any(|arg| arg == "-File"));
    }

    #[test]
    fn automatic_file_transport_spawn_failure_returns_transport_exit_code() {
        let prepared = prepare_powershell_transport(
            OsStr::new("rtk-test-missing-powershell-host.exe"),
            &"x".repeat(8 * 1024 + 1),
            PowerShellExecutionPolicy::BypassNotNeeded,
        )
        .unwrap();
        assert!(prepared.temp_script.is_some());

        let code = spawn_prepared_powershell(
            OsString::from("rtk-test-missing-powershell-host.exe"),
            prepared,
            "PowerShell transport",
        )
        .unwrap();

        assert_eq!(code, 2);
    }

    #[test]
    fn automatic_file_transport_spawn_failure_names_the_transport_stage() {
        let message = format_powershell_file_transport_spawn_error(
            OsStr::new("rtk-test-missing-powershell-host.exe"),
            &std::io::Error::from(std::io::ErrorKind::NotFound),
        );

        assert!(message.starts_with("rtk: PowerShell file transport: spawn"));
    }

    #[test]
    fn execution_policy_classification_requires_bypass_only_for_restricted_or_all_signed() {
        assert_eq!(
            classify_execution_policy("Restricted"),
            PowerShellExecutionPolicy::BypassNeeded
        );
        assert_eq!(
            classify_execution_policy("AllSigned\r\n"),
            PowerShellExecutionPolicy::BypassNeeded
        );
        assert_eq!(
            classify_execution_policy("RemoteSigned"),
            PowerShellExecutionPolicy::BypassNotNeeded
        );
    }

    #[cfg(windows)]
    #[test]
    fn execution_policy_probe_cleanup_terminates_and_reaps_child() {
        let mut child = Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 > NUL"])
            .spawn()
            .unwrap();

        terminate_execution_policy_probe(&mut child);

        assert!(child.try_wait().unwrap().is_some());
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
    fn explicit_ps1_uses_file_without_automatic_bypass() {
        let args = powershell_file_args(
            Path::new("tool.ps1"),
            &[OsString::from("a b")],
            PowerShellExecutionPolicy::BypassNotNeeded,
        );
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

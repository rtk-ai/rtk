//! Public PowerShell host invocation and conservative output filtering.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::io::{self, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use super::adapters;
use super::catalog::{self, AdapterStrategy};
use super::parser::{parse_expression, ParsedScript};
use super::transport::OutputSpool;

static PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerShellHost {
    WindowsPowerShell,
    Pwsh,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDialect {
    Desktop51,
    Pwsh,
}

impl PowerShellHost {
    fn executable(self) -> &'static str {
        match self {
            Self::WindowsPowerShell => "powershell.exe",
            Self::Pwsh => "pwsh.exe",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::WindowsPowerShell => "powershell",
            Self::Pwsh => "pwsh",
        }
    }

    pub fn dialect(self) -> HostDialect {
        match self {
            Self::WindowsPowerShell => HostDialect::Desktop51,
            Self::Pwsh => HostDialect::Pwsh,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Invocation {
    Passthrough(Vec<OsString>),
    Command {
        host_args: Vec<OsString>,
        expression: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RewriteDecision {
    Filter { adapter: AdapterStrategy },
    Raw { reason: &'static str },
}

/// Separate host flags from a one-shot source expression without interpreting
/// the expression itself. Explicit transport modes stay byte-for-byte native.
pub fn prepare_invocation(args: &[OsString]) -> Invocation {
    if args.is_empty() {
        return Invocation::Passthrough(Vec::new());
    }

    // -Command/-c is filterable when it has exactly one source argument. The
    // other execution modes have semantics that cannot be reconstructed safely.
    if let Some(command_index) = find_command_mode(args) {
        // `-Command -` consumes the script from stdin. Capturing the child
        // would close that transport, so preserve the complete native argv.
        if args
            .get(command_index + 1)
            .is_some_and(|expression| expression == "-")
        {
            return Invocation::Passthrough(args.to_vec());
        }
        if command_index + 1 < args.len() && command_index + 2 == args.len() {
            return Invocation::Command {
                host_args: args[..command_index].to_vec(),
                expression: args[command_index + 1].to_string_lossy().into_owned(),
            };
        }
        return Invocation::Passthrough(args.to_vec());
    }
    if contains_opaque_execution_mode(args) {
        return Invocation::Passthrough(args.to_vec());
    }

    let mut host_args = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let token = args[index].to_string_lossy();
        let lower = token.to_ascii_lowercase();
        if !is_host_option(&lower) {
            break;
        }
        host_args.push(args[index].clone());
        if host_option_takes_value(&lower) {
            if let Some(value) = args.get(index + 1) {
                host_args.push(value.clone());
                index += 1;
            }
        }
        index += 1;
    }

    let expression_args = &args[index..];
    if expression_args.is_empty() {
        return Invocation::Passthrough(host_args);
    }
    Invocation::Command {
        host_args,
        expression: reconstruct_expression(expression_args),
    }
}

#[allow(dead_code)]
pub fn classify(parsed: &ParsedScript, host_args: &[OsString]) -> RewriteDecision {
    classify_expression(parsed, "", host_args)
}

pub fn classify_expression(
    parsed: &ParsedScript,
    source: &str,
    host_args: &[OsString],
) -> RewriteDecision {
    if parsed.is_opaque() {
        return RewriteDecision::Raw {
            reason: "parser marked the source opaque",
        };
    }
    if source.len() > 32 * 1024 {
        return RewriteDecision::Raw {
            reason: "source exceeds safe wrapper command-line limits",
        };
    }
    if host_args.iter().enumerate().any(|(index, arg)| {
        let value = arg.to_string_lossy().to_ascii_lowercase();
        let paired_value = host_args
            .get(index + 1)
            .map(|next| next.to_string_lossy().to_ascii_lowercase());
        matches!(
            value.as_str(),
            "-noexit"
                | "-noe"
                | "-login"
                | "-interactive"
                | "-sshservermode"
                | "-file"
                | "-f"
                | "-encodedcommand"
                | "-e"
                | "-ec"
                | "-encodedarguments"
                | "-encodedargs"
                | "-stdin"
        ) || value.starts_with("-outputformat:xml")
            || value.starts_with("-inputformat:")
            || ((value == "-outputformat" || value == "-inputformat")
                && paired_value.as_deref().is_some_and(|next| next == "xml"))
    }) {
        return RewriteDecision::Raw {
            reason: "host flags select a long-running or machine transport mode",
        };
    }
    if contains_long_running_parameter(source) {
        return RewriteDecision::Raw {
            reason: "long-running or background parameters are native-only",
        };
    }
    let lowered_source = source.to_ascii_lowercase();
    if lowered_source.contains("read-host")
        || lowered_source.contains("readkey")
        || lowered_source.contains("$input")
    {
        return RewriteDecision::Raw {
            reason: "stdin and interactive reads remain native-only",
        };
    }
    if contains_non_success_stream(source) {
        return RewriteDecision::Raw {
            reason: "auxiliary stream producers retain native ordering",
        };
    }
    let names = parsed.command_names();
    if contains_remoting_command(&names) {
        return RewriteDecision::Raw {
            reason: "remoting commands retain native interactive transport",
        };
    }
    if contains_dynamic_invocation(&names, source) {
        return RewriteDecision::Raw {
            reason: "dynamic and dot-sourced scripts remain opaque",
        };
    }
    if names.len() != 1 {
        return RewriteDecision::Raw {
            reason: "compound expressions retain native state and stream ordering",
        };
    }
    let command = &names[0];
    if matches!(
        crate::core::utils::resolve_host_command(command),
        crate::core::utils::HostCommand::Executable(_)
    ) {
        return RewriteDecision::Raw {
            reason: "resolved native applications and scripts remain opaque",
        };
    }
    if command.contains('/')
        || command.contains('\\')
        || command.ends_with(".exe")
        || command.ends_with(".com")
        || command.ends_with(".bat")
        || command.ends_with(".cmd")
    {
        return RewriteDecision::Raw {
            reason: "native executable output is opaque",
        };
    }
    match catalog::strategy_for(command) {
        AdapterStrategy::Identity => RewriteDecision::Raw {
            reason: "identity commands must retain their native display",
        },
        adapter => RewriteDecision::Filter { adapter },
    }
}

pub fn classify_for_host(
    host: PowerShellHost,
    parsed: &ParsedScript,
    source: &str,
    host_args: &[OsString],
) -> RewriteDecision {
    let _dialect = host.dialect();
    classify_expression(parsed, source, host_args)
}

pub fn run(host: PowerShellHost, args: &[OsString]) -> Result<i32> {
    if !cfg!(windows) {
        bail!(
            "rtk {} is only supported on Windows 10 and 11",
            host.display_name()
        );
    }
    let executable = crate::core::utils::resolve_binary(host.executable())
        .with_context(|| format!("Failed to resolve {} from PATH", host.executable()))?;
    let invocation = prepare_invocation(args);
    match invocation {
        Invocation::Passthrough(arguments) => run_passthrough(&executable, host, &arguments),
        Invocation::Command {
            host_args,
            expression,
        } => {
            let parsed = parse_expression(&expression);
            let decision = classify_for_host(host, &parsed, &expression, &host_args);
            if filtering_requested() {
                if let RewriteDecision::Filter { adapter } = decision {
                    return run_filtered_command(
                        &executable,
                        host,
                        &host_args,
                        &expression,
                        parsed
                            .command_names()
                            .first()
                            .map(String::as_str)
                            .unwrap_or("unknown"),
                        adapter,
                    );
                }
            }
            run_command(&executable, host, &host_args, &expression)
        }
    }
}

/// Transport entry point used by hidden helpers and tests. It deliberately
/// bypasses the RTK router and executes one already-decoded source expression
/// in the selected native host.
pub fn run_raw(host: PowerShellHost, expression: &str) -> Result<i32> {
    if !cfg!(windows) {
        bail!(
            "rtk {} is only supported on Windows 10 and 11",
            host.display_name()
        );
    }
    let executable = crate::core::utils::resolve_binary(host.executable())
        .with_context(|| format!("Failed to resolve {} from PATH", host.executable()))?;
    run_command(&executable, host, &[], expression)
}

fn run_passthrough(executable: &PathBuf, host: PowerShellHost, args: &[OsString]) -> Result<i32> {
    // nosemgrep: dynamic-command-execution -- host executable is resolved from the native PATH.
    let status = Command::new(executable)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {}", host.executable()))?;
    Ok(crate::core::utils::exit_code_from_status(
        &status,
        host.display_name(),
    ))
}

fn run_command(
    executable: &PathBuf,
    host: PowerShellHost,
    host_args: &[OsString],
    expression: &str,
) -> Result<i32> {
    // nosemgrep: dynamic-command-execution -- host executable is resolved from the native PATH.
    let status = Command::new(executable)
        .args(host_args)
        .arg("-Command")
        .arg(expression)
        .env_remove("RTK_POWERSHELL_FILTER")
        .status()
        .with_context(|| format!("Failed to execute {}", host.executable()))?;
    Ok(crate::core::utils::exit_code_from_status(
        &status,
        host.display_name(),
    ))
}

fn run_filtered_command(
    executable: &PathBuf,
    host: PowerShellHost,
    host_args: &[OsString],
    expression: &str,
    command_name: &str,
    strategy: AdapterStrategy,
) -> Result<i32> {
    let output_spool = match OutputSpool::create() {
        Ok(spool) => spool,
        Err(_) => return run_command(executable, host, host_args, expression),
    };
    let mode_spool = match OutputSpool::create() {
        Ok(spool) => spool,
        Err(_) => return run_command(executable, host, host_args, expression),
    };
    let execution_expression =
        runtime_probe_expression(expression, command_name, mode_spool.path());
    // nosemgrep: dynamic-command-execution -- host executable is resolved from the native PATH.
    let mut command = Command::new(executable);
    command
        .args(host_args)
        .arg("-Command")
        .arg(&execution_expression)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("RTK_POWERSHELL_FILTER");
    let (status, captured) = match capture_ordered_output(&mut command) {
        Ok(output) => output,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::InvalidInput
                    | io::ErrorKind::PermissionDenied
            ) =>
        {
            // spawn() failed before a child existed, so the native fallback
            // cannot rerun an already-started command.
            return run_command(executable, host, host_args, expression);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to execute {}", host.executable()))
        }
    };

    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    if captured.overflowed {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    }
    let ordered_output = &captured.chunks;
    let mut success_stream = Vec::new();
    let mut auxiliary_stream_seen = false;
    for chunk in ordered_output {
        match chunk.stream {
            CapturedStream::Stdout => success_stream.extend_from_slice(&chunk.bytes),
            CapturedStream::Stderr => auxiliary_stream_seen = true,
        }
    }
    let probe_mode = mode_spool.read_utf8().ok();
    // A raw probe result, an auxiliary stream, or a failed command is replayed
    // chunk-by-chunk to preserve the observed native stream ordering.
    if probe_mode.as_deref() != Some("filter") || !status.success() || auxiliary_stream_seen {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    }

    if output_spool.write(&success_stream).is_err() {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    }
    let Ok(raw_owned) = output_spool.read_utf8() else {
        // Never compact undecodable bytes. Replay the original captured bytes
        // so Desktop PowerShell's native code-page output remains intact.
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    };
    let raw = raw_owned.as_str();
    let adapter_name = match strategy {
        AdapterStrategy::Specialized(name) => name,
        AdapterStrategy::Generic => "generic",
        AdapterStrategy::Identity => "identity",
    };
    let Some(filtered) = adapters::filter_output(adapter_name, raw) else {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    };
    let Some(reservation) = crate::core::tee::reserve_lossless_tee(raw, host.display_name()) else {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    };
    let Some(commit) =
        crate::core::tee::commit_lossless_if_better_for_powershell(raw, &filtered, reservation)
    else {
        captured.replay(&mut stdout, &mut stderr);
        return Ok(crate::core::utils::exit_code_from_status(
            &status,
            host.display_name(),
        ));
    };
    stdout.write_all(commit.as_bytes()).ok();
    crate::core::tracking::TimedExecution::start().track(
        expression,
        &format!("rtk {}", host.display_name()),
        raw,
        std::str::from_utf8(commit.as_bytes()).unwrap_or_default(),
    );
    Ok(crate::core::utils::exit_code_from_status(
        &status,
        host.display_name(),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapturedStream {
    Stdout,
    Stderr,
}

#[derive(Debug)]
struct OutputChunk {
    stream: CapturedStream,
    bytes: Vec<u8>,
}

const MAX_CAPTURED_BYTES: usize = 64 * 1024;

struct OrderedCaptureSpool {
    spool: OutputSpool,
}

impl OrderedCaptureSpool {
    fn create() -> io::Result<Self> {
        Ok(Self {
            spool: OutputSpool::create()?,
        })
    }

    fn append(&self, stream: CapturedStream, bytes: &[u8]) -> io::Result<()> {
        let tag = match stream {
            CapturedStream::Stdout => 0_u8,
            CapturedStream::Stderr => 1_u8,
        };
        let length = u64::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "capture chunk too large"))?;
        let mut record = Vec::with_capacity(1 + 8 + bytes.len());
        record.push(tag);
        record.extend_from_slice(&length.to_le_bytes());
        record.extend_from_slice(bytes);
        self.spool.append(&record)
    }

    fn replay(&self, stdout: &mut impl Write, stderr: &mut impl Write) -> io::Result<()> {
        let mut file = std::fs::File::open(self.spool.path())?;
        loop {
            let mut tag = [0_u8; 1];
            match file.read_exact(&mut tag) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(error) => return Err(error),
            }
            let mut length_bytes = [0_u8; 8];
            file.read_exact(&mut length_bytes)?;
            let length = usize::try_from(u64::from_le_bytes(length_bytes)).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "capture chunk too large")
            })?;
            let mut bytes = vec![0_u8; length];
            file.read_exact(&mut bytes)?;
            match tag[0] {
                0 => stdout.write_all(&bytes)?,
                1 => stderr.write_all(&bytes)?,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unknown capture stream",
                    ))
                }
            }
        }
        Ok(())
    }
}

struct CapturedOutput {
    chunks: Vec<OutputChunk>,
    spool: OrderedCaptureSpool,
    overflowed: bool,
}

impl CapturedOutput {
    fn replay(&self, stdout: &mut impl Write, stderr: &mut impl Write) {
        if self.overflowed {
            let _ = self.spool.replay(stdout, stderr);
        } else {
            replay_ordered_output(&self.chunks, stdout, stderr);
        }
    }
}

fn capture_ordered_output(
    command: &mut Command,
) -> io::Result<(std::process::ExitStatus, CapturedOutput)> {
    let spool = OrderedCaptureSpool::create()?;
    let mut child = command.spawn()?;
    let stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("PowerShell stdout pipe unavailable"))?;
    let stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("PowerShell stderr pipe unavailable"))?;
    let (sender, receiver) = mpsc::channel();
    let _stdout_thread = spawn_capture_thread(stdout_pipe, CapturedStream::Stdout, sender.clone());
    let _stderr_thread = spawn_capture_thread(stderr_pipe, CapturedStream::Stderr, sender);
    let mut chunks = Vec::new();
    let mut captured_bytes = 0usize;
    let mut overflowed = false;
    for chunk in receiver {
        spool.append(chunk.stream, &chunk.bytes)?;
        if !overflowed {
            if let Some(total) = captured_bytes.checked_add(chunk.bytes.len()) {
                if total <= MAX_CAPTURED_BYTES {
                    captured_bytes = total;
                    chunks.push(chunk);
                    continue;
                }
            }
            overflowed = true;
            chunks.clear();
        }
    }
    let status = child.wait()?;
    Ok((
        status,
        CapturedOutput {
            chunks,
            spool,
            overflowed,
        },
    ))
}

fn spawn_capture_thread<R>(
    mut reader: R,
    stream: CapturedStream,
    sender: Sender<OutputChunk>,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(length) => {
                    if sender
                        .send(OutputChunk {
                            stream,
                            bytes: buffer[..length].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn replay_ordered_output(chunks: &[OutputChunk], stdout: &mut impl Write, stderr: &mut impl Write) {
    for chunk in chunks {
        match chunk.stream {
            CapturedStream::Stdout => {
                stdout.write_all(&chunk.bytes).ok();
            }
            CapturedStream::Stderr => {
                stderr.write_all(&chunk.bytes).ok();
            }
        }
    }
}

fn runtime_probe_expression(
    expression: &str,
    command_name: &str,
    mode_path: &std::path::Path,
) -> String {
    let sequence = PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let identifier = format!("__rtk_probe_{timestamp:x}_{sequence:x}");
    let probe_variable = format!("${identifier}");
    let mode_variable = format!("${identifier}_mode");
    let saved_error_variable = format!("${identifier}_saved_error");
    let error_item_variable = format!("${identifier}_error_item");
    let quoted_name = command_name.replace('\'', "''");
    let mode_path = quote_argument(&mode_path.to_string_lossy());
    let metadata_condition = catalog::lookup(command_name)
        .map(|metadata| {
            format!(
                "{probe_variable}.CommandType -eq 'Cmdlet' -and {probe_variable}.Name -ieq '{}' -and {probe_variable}.ModuleName -ieq '{}'",
                metadata.canonical_name.replace('\'', "''"),
                metadata.module.replace('\'', "''")
            )
        })
        .unwrap_or_else(|| {
            format!(
                "{probe_variable}.CommandType -in @('Cmdlet','Function','Filter')"
            )
        });
    format!(
        "{saved_error_variable}=@($Error); {probe_variable}=Microsoft.PowerShell.Core\\Get-Command -Name '{quoted_name}' -ErrorAction SilentlyContinue; while ({probe_variable} -and {probe_variable}.CommandType -eq 'Alias') {{ {probe_variable}=Microsoft.PowerShell.Core\\Get-Command -Name {probe_variable}.Definition -ErrorAction SilentlyContinue }}; {mode_variable} = if ({metadata_condition}) {{ 'filter' }} else {{ 'raw' }}; $Error.Clear(); foreach ({error_item_variable} in {saved_error_variable}) {{ [void]$Error.Add({error_item_variable}) }}; [IO.File]::WriteAllText({mode_path}, {mode_variable}, [Text.UTF8Encoding]::new($false)); {expression}"
    )
}

fn contains_long_running_parameter(source: &str) -> bool {
    source
        .split(|character: char| character.is_whitespace() || character == ',' || character == ';')
        .map(|token| token.trim_matches(['(', ')', '{', '}', '[', ']']))
        .filter_map(|token| token.strip_prefix('-'))
        .map(|token| token.split_once(':').map_or(token, |(name, _)| name))
        .map(str::to_ascii_lowercase)
        .any(|token| {
            ["asjob", "wait", "follow", "watch", "continuous"]
                .iter()
                .any(|name| name.starts_with(&token))
        })
}

fn contains_non_success_stream(source: &str) -> bool {
    let lowered = source.to_ascii_lowercase();
    [
        "write-error",
        "write-warning",
        "write-verbose",
        "write-debug",
        "write-information",
        "write-progress",
        "2>",
        "3>",
        "4>",
        "5>",
        "6>",
        ">&",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

fn contains_remoting_command(names: &[String]) -> bool {
    names.iter().any(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "connect-pssession"
                | "debug-job"
                | "enter-pshostprocess"
                | "enter-pssession"
                | "exit-pssession"
                | "invoke-command"
                | "new-pssession"
                | "receive-pssession"
        )
    })
}

fn contains_dynamic_invocation(names: &[String], source: &str) -> bool {
    let first = names
        .first()
        .map(|name| name.to_ascii_lowercase())
        .unwrap_or_default();
    first == "."
        || matches!(first.as_str(), "iex" | "invoke-expression")
        || source.trim_start().starts_with(". ")
        || source.trim_start().starts_with(".\t")
}

fn filtering_requested() -> bool {
    io::stdout().is_terminal()
        || std::env::var("RTK_POWERSHELL_FILTER")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        || matches!(
            std::env::var("RTK_OUTPUT_AUDIENCE").ok().as_deref(),
            Some("agent")
        )
}

fn contains_opaque_execution_mode(args: &[OsString]) -> bool {
    find_initial_mode(args).is_some_and(|mode| {
        let mode = mode.to_ascii_lowercase();
        matches!(
            mode.as_str(),
            "-file"
                | "-f"
                | "-encodedcommand"
                | "-e"
                | "-ec"
                | "-encodedarguments"
                | "-encodedargs"
                | "-commandwithargs"
                | "-stdin"
        )
    })
}

fn is_command_mode(token: &str) -> bool {
    matches!(token.to_ascii_lowercase().as_str(), "-command" | "-c")
}

fn find_command_mode(args: &[OsString]) -> Option<usize> {
    find_initial_mode_with_index(args)
        .and_then(|(index, mode)| is_command_mode(mode).then_some(index))
}

fn find_initial_mode(args: &[OsString]) -> Option<&str> {
    find_initial_mode_with_index(args).map(|(_, mode)| mode)
}

fn find_initial_mode_with_index(args: &[OsString]) -> Option<(usize, &str)> {
    let mut index = 0;
    while index < args.len() {
        let token = args[index].to_string_lossy();
        let lower = token.to_ascii_lowercase();
        if is_host_option(&lower) {
            index += 1 + usize::from(host_option_takes_value(&lower));
            continue;
        }
        if lower.starts_with('-') {
            return Some((index, args[index].to_str().unwrap_or_default()));
        }
        break;
    }
    None
}

fn is_host_option(token: &str) -> bool {
    matches!(
        token,
        "-nologo"
            | "-nol"
            | "-noexit"
            | "-noe"
            | "-noprofile"
            | "-nop"
            | "-noninteractive"
            | "-noni"
            | "-sta"
            | "-mta"
            | "-version"
            | "-v"
            | "-inputformat"
            | "-in"
            | "-outputformat"
            | "-out"
            | "-windowstyle"
            | "-w"
            | "-configurationname"
            | "-config"
            | "-executionpolicy"
            | "-ep"
            | "-psconsolefile"
            | "-login"
            | "-interactive"
            | "-noprofileloadtime"
            | "-sshservermode"
            | "-settingsfile"
            | "-configurationfile"
            | "-custompipename"
            | "-workingdirectory"
            | "-wd"
            | "-help"
            | "-?"
            | "/?"
    ) || token.starts_with("-inputformat:")
        || token.starts_with("-in:")
        || token.starts_with("-outputformat:")
        || token.starts_with("-out:")
        || token.starts_with("-executionpolicy:")
        || token.starts_with("-ep:")
        || token.starts_with("-configurationname:")
        || token.starts_with("-config:")
        || token.starts_with("-windowstyle:")
        || token.starts_with("-w:")
        || token.starts_with("-psconsolefile:")
        || token.starts_with("-workingdirectory:")
        || token.starts_with("-wd:")
}

fn host_option_takes_value(token: &str) -> bool {
    matches!(
        token,
        "-version"
            | "-v"
            | "-inputformat"
            | "-in"
            | "-outputformat"
            | "-out"
            | "-windowstyle"
            | "-w"
            | "-configurationname"
            | "-config"
            | "-executionpolicy"
            | "-ep"
            | "-psconsolefile"
            | "-settingsfile"
            | "-configurationfile"
            | "-custompipename"
            | "-workingdirectory"
            | "-wd"
    )
}

fn reconstruct_expression(args: &[OsString]) -> String {
    args.iter()
        .enumerate()
        .map(|(index, arg)| {
            let value = arg.to_string_lossy();
            if index == 0 || value.starts_with('-') {
                value.into_owned()
            } else {
                quote_argument(&value)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[allow(dead_code)]
fn resolve_host_path(host: PowerShellHost) -> Result<PathBuf> {
    crate::core::utils::resolve_binary(host.executable())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn command_mode_keeps_a_single_expression_filterable() {
        assert_eq!(
            prepare_invocation(&args(&["-NoProfile", "-Command", "Get-Process"])),
            Invocation::Command {
                host_args: args(&["-NoProfile"]),
                expression: "Get-Process".to_string(),
            }
        );
    }

    #[test]
    fn command_stdin_sentinel_stays_native_passthrough() {
        let input = args(&["-Command", "-"]);

        assert_eq!(prepare_invocation(&input), Invocation::Passthrough(input));
    }

    #[test]
    fn native_startup_abbreviations_stay_with_the_host() {
        assert_eq!(
            prepare_invocation(&args(&[
                "-NoP",
                "-NonI",
                "-NoE",
                "-NoL",
                "-WD",
                "C:\\work",
                "-Command",
                "Write-Output OK",
            ])),
            Invocation::Command {
                host_args: args(&["-NoP", "-NonI", "-NoE", "-NoL", "-WD", "C:\\work"]),
                expression: "Write-Output OK".to_string(),
            }
        );
    }

    #[test]
    fn abbreviated_long_running_parameters_fail_open() {
        for parameter in [
            "-W",
            "-Wa",
            "-Wai",
            "-Fo",
            "-Fol",
            "-Wat",
            "-Wat:$true",
            "-Cont",
            "-AsJob:$true",
        ] {
            let source = format!("Get-Process {parameter}");
            let parsed = parse_expression(&source);
            assert!(
                matches!(
                    classify_expression(&parsed, &source, &[]),
                    RewriteDecision::Raw { .. }
                ),
                "{parameter} should remain native-only"
            );
        }
    }

    #[test]
    fn encoded_and_file_modes_are_exact_passthrough() {
        let input = args(&["-File", "script.ps1"]);
        assert_eq!(prepare_invocation(&input), Invocation::Passthrough(input));
    }

    #[test]
    fn positional_arguments_are_quoted_as_data() {
        assert_eq!(
            prepare_invocation(&args(&["Write-Output", "a & b", "O'Reilly"])),
            Invocation::Command {
                host_args: Vec::new(),
                expression: "Write-Output 'a & b' 'O''Reilly'".to_string(),
            }
        );
    }

    #[test]
    fn command_like_argument_inside_expression_is_not_a_host_flag() {
        assert!(matches!(
            prepare_invocation(&args(&["Write-Output", "-Command"])),
            Invocation::Command { expression, .. } if expression == "Write-Output -Command"
        ));
    }

    #[test]
    fn compound_and_native_commands_fail_open() {
        let parsed = parse_expression("Get-Process | sort");
        assert!(matches!(
            classify(&parsed, &[]),
            RewriteDecision::Raw { .. }
        ));
        let parsed = parse_expression("cargo --version");
        assert!(matches!(
            classify(&parsed, &[]),
            RewriteDecision::Raw { .. }
        ));
        let parsed = parse_expression("C:\\Windows\\System32\\whoami.exe");
        assert!(matches!(
            classify(&parsed, &[]),
            RewriteDecision::Raw { .. }
        ));
    }

    #[test]
    fn long_running_parameters_fail_open() {
        let parsed = parse_expression("Get-Process -AsJob");
        assert!(matches!(
            classify_expression(&parsed, "Get-Process -AsJob", &[]),
            RewriteDecision::Raw { .. }
        ));
    }

    #[test]
    fn runtime_probe_is_same_runspace_and_uses_a_randomized_identifier() {
        let wrapped = runtime_probe_expression(
            "Get-Process",
            "Get-Process",
            PathBuf::from("C:\\Temp\\rtk-mode.txt").as_path(),
        );
        assert!(wrapped.contains("Microsoft.PowerShell.Core\\Get-Command"));
        assert!(wrapped.contains("Get-Process"));
        assert!(wrapped.contains("__rtk_probe_"));
        assert!(wrapped.contains("Microsoft.PowerShell.Management"));
        assert!(wrapped.contains("WriteAllText"));
        assert!(wrapped.contains("$Error.Clear"));
        assert!(wrapped.contains(" -and $__rtk_probe_"));
    }

    #[test]
    fn host_routes_expose_distinct_dialects() {
        assert_eq!(
            PowerShellHost::WindowsPowerShell.dialect(),
            HostDialect::Desktop51
        );
        assert_eq!(PowerShellHost::Pwsh.dialect(), HostDialect::Pwsh);
    }

    #[test]
    fn xml_output_modes_fail_open_even_when_value_is_separate() {
        let parsed = parse_expression("Get-Process");
        assert!(matches!(
            classify_for_host(
                PowerShellHost::Pwsh,
                &parsed,
                "Get-Process",
                &args(&["-OutputFormat", "XML"])
            ),
            RewriteDecision::Raw { .. }
        ));
    }

    #[test]
    fn stdin_reads_fail_open() {
        let parsed = parse_expression("Read-Host Name");
        assert!(matches!(
            classify_expression(&parsed, "Read-Host Name", &[]),
            RewriteDecision::Raw { .. }
        ));
    }

    #[test]
    fn interactive_remoting_commands_fail_open() {
        for source in [
            "Enter-PSSession -ComputerName server",
            "Invoke-Command -ComputerName server -ScriptBlock { Get-Process }",
        ] {
            let parsed = parse_expression(source);
            assert!(
                matches!(
                    classify_expression(&parsed, source, &[]),
                    RewriteDecision::Raw { .. }
                ),
                "{source} should preserve native remoting behavior"
            );
        }
    }

    #[test]
    fn dynamic_invocation_and_dot_sourcing_fail_open() {
        for source in [
            ". .\\profile-script.ps1",
            "Invoke-Expression $source",
            "iex $source",
        ] {
            let parsed = parse_expression(source);
            assert!(
                matches!(
                    classify_expression(&parsed, source, &[]),
                    RewriteDecision::Raw { .. }
                ),
                "{source} should remain opaque"
            );
        }
    }

    #[test]
    fn auxiliary_stream_producers_fail_open() {
        for source in ["Get-Process; Write-Warning warning", "Get-Process 2>&1"] {
            let parsed = parse_expression(source);
            assert!(
                matches!(
                    classify_expression(&parsed, source, &[]),
                    RewriteDecision::Raw { .. }
                ),
                "{source} should preserve native stream behavior"
            );
        }
    }

    #[test]
    fn ordered_replay_keeps_stdout_and_stderr_chunk_order() {
        let chunks = vec![
            OutputChunk {
                stream: CapturedStream::Stdout,
                bytes: b"out-1".to_vec(),
            },
            OutputChunk {
                stream: CapturedStream::Stderr,
                bytes: b"err-1".to_vec(),
            },
            OutputChunk {
                stream: CapturedStream::Stdout,
                bytes: b"out-2".to_vec(),
            },
        ];
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        replay_ordered_output(&chunks, &mut stdout, &mut stderr);
        assert_eq!(stdout, b"out-1out-2");
        assert_eq!(stderr, b"err-1");
    }

    #[cfg(windows)]
    #[test]
    fn large_capture_does_not_retain_the_entire_success_stream_in_memory() {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::Out.Write(('x' * 131072))",
        ]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let (_status, captured) = capture_ordered_output(&mut command).expect("capture output");
        let captured_bytes: usize = captured.chunks.iter().map(|chunk| chunk.bytes.len()).sum();

        assert!(
            captured_bytes <= 64 * 1024,
            "captured {captured_bytes} bytes instead of switching to disk-backed replay"
        );
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        captured.replay(&mut stdout, &mut stderr);
        assert_eq!(stdout.len(), 131072);
        assert!(stderr.is_empty());
    }
}

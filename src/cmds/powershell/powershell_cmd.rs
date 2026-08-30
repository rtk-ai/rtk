//! PowerShell cmdlet routing and filtering.
//!
//! `rtk powershell <Cmdlet> [args...]` runs a PowerShell cmdlet through
//! `pwsh`/`powershell.exe -NoProfile -NonInteractive -Command <script>` and
//! condenses its console-formatted output the same way RTK condenses
//! git/cargo output: keep the header plus a capped number of data rows for
//! tabular cmdlets (`Get-Process`, `Get-Service`, ...), or keep only the
//! "essential" properties for single-object list-formatted cmdlets like
//! `Get-ComputerInfo` and `Test-NetConnection`.
//!
//! # Injection safety
//!
//! Two independent boundaries keep untrusted input from being executed as
//! PowerShell code:
//!
//! 1. **Cmdlet names.** Names dispatched from the known match arms in
//!    [`run`] are rtk-controlled string literals — never derived from user
//!    input. The one path where the "cmdlet name" itself IS user input is the
//!    unknown-cmdlet passthrough fallback; [`validate_cmdlet_name`]
//!    allowlists it against `^[A-Za-z][A-Za-z0-9-]*$` (the shape of every
//!    real cmdlet, function, and alias name) before it can reach a script,
//!    and rejects anything else outright. A command *name* is a token, not a
//!    data value — quoting/escaping it doesn't make it safely invocable, so
//!    it's allowlisted instead of sanitized.
//! 2. **Argument values.** No argument value is ever quoted-and-interpolated
//!    into the `-Command` script text. [`build_powershell_script`] embeds a
//!    literal token directly only when it matches `^-[A-Za-z][A-Za-z0-9]*$`
//!    (a bare switch/parameter name like `-Recurse` or `-Path`, containing no
//!    character PowerShell's tokenizer treats specially — the allowlist
//!    itself is the safety property, not escaping). Every other argument is
//!    passed to the child process as an environment variable
//!    (`RTK_PS_ARG_<n>`, set by [`powershell_command`]) and referenced in the
//!    script as `$env:RTK_PS_ARG_<n>`; PowerShell expands `$env:` values as
//!    inert string data with no re-parsing, so there is no tokenizer edge
//!    case (smart quotes, backticks, `$(...)`, `#`, `;`, ...) left to
//!    neutralize.
//!
//! Cmdlets whose output is already minimal (`Test-Path`, `Remove-Item`, ...)
//! or that are interactive/side-effecting (`Start-Process`, `Set-Location`,
//! ...) are passed straight through unfiltered — like `rtk proxy` — rather
//! than fabricate a "filter" with nothing meaningful to condense. Unknown
//! cmdlets fall back to the same passthrough path (subject to the cmdlet-name
//! allowlist above) so RTK never blocks a command it doesn't specifically
//! recognize.

use crate::core::runner::{self, RunMode, RunOptions};
use crate::core::truncate::{CAP_INVENTORY, CAP_LIST};
use crate::core::utils::{join_with_overflow, resolved_command, tool_exists};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::process::Command;

lazy_static! {
    /// Every real PowerShell cmdlet, function, and alias name matches this
    /// shape (`Verb-Noun`, or a short alias like `ls`/`gci`). Anything else
    /// reaching [`run_passthrough`] is untrusted input used as a command
    /// *token* — quoting a token doesn't make it safely invocable the way
    /// quoting a data value does, so it's rejected outright rather than
    /// sanitized. See [`validate_cmdlet_name`].
    static ref VALID_CMDLET_NAME: Regex = Regex::new(r"^[A-Za-z][A-Za-z0-9-]*$").unwrap();

    /// A parameter-name TOKEN in `-Name` form: a switch (`-Recurse`) or a
    /// named parameter immediately followed by its value as a separate
    /// argument (`-Path`, then a value). This shape contains no character
    /// PowerShell's tokenizer treats specially (no quotes, `$`, backticks,
    /// `;`, `#`, whitespace, ...), so — unlike a data VALUE — it's safe to
    /// embed directly in the script text: the allowlist itself is the safety
    /// property. Anything that doesn't match (including a string merely
    /// *starting* with `-` followed by special characters) is treated as a
    /// data value and routed through an environment variable instead. See
    /// [`build_powershell_script`].
    static ref SAFE_FLAG_TOKEN: Regex = Regex::new(r"^-[A-Za-z][A-Za-z0-9]*$").unwrap();
}

// ---------------------------------------------------------------------------
// Entry point / dispatch
// ---------------------------------------------------------------------------

/// Route a PowerShell cmdlet invocation to its filter (or passthrough).
///
/// `args[0]` is the cmdlet name (e.g. `Get-ChildItem`, or a real PowerShell
/// alias like `gci`); `args[1..]` are the cmdlet's own arguments.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let cmdlet = args
        .first()
        .context("rtk powershell: missing cmdlet name (e.g. `rtk powershell Get-ChildItem`)")?
        .as_str();
    let rest = &args[1..];

    if verbose > 0 {
        eprintln!("rtk powershell: {} {}", cmdlet, rest.join(" "));
    }

    match cmdlet {
        "Get-ChildItem" | "gci" | "ls" | "dir" => run_table_cmdlet(
            "Get-ChildItem",
            "powershell Get-ChildItem",
            rest,
            CHILDITEM_PIPELINE,
            filter_get_childitem,
        ),
        "Get-Item" | "gi" => run_table_cmdlet(
            "Get-Item",
            "powershell Get-Item",
            rest,
            ITEM_PIPELINE,
            filter_get_item,
        ),
        "Get-Content" | "gc" | "cat" | "type" => run_plain_cmdlet(
            "Get-Content",
            "powershell Get-Content",
            rest,
            filter_get_content,
        ),
        "Get-Process" | "gps" | "ps" => run_table_cmdlet(
            "Get-Process",
            "powershell Get-Process",
            rest,
            PROCESS_PIPELINE,
            filter_get_process,
        ),
        "Get-Service" | "gsv" => run_table_cmdlet(
            "Get-Service",
            "powershell Get-Service",
            rest,
            SERVICE_PIPELINE,
            filter_get_service,
        ),
        "Select-String" | "sls" => run_plain_cmdlet(
            "Select-String",
            "powershell Select-String",
            rest,
            filter_select_string,
        ),
        "Get-WinEvent" | "gwe" => run_table_cmdlet(
            "Get-WinEvent",
            "powershell Get-WinEvent",
            rest,
            WINEVENT_PIPELINE,
            filter_get_winevent,
        ),
        "Get-ItemProperty" | "gp" => run_plain_cmdlet(
            "Get-ItemProperty",
            "powershell Get-ItemProperty",
            rest,
            filter_get_itemproperty,
        ),
        "Test-NetConnection" | "tnc" => run_plain_cmdlet(
            "Test-NetConnection",
            "powershell Test-NetConnection",
            rest,
            filter_test_netconnection,
        ),
        "Get-NetAdapter" => run_table_cmdlet(
            "Get-NetAdapter",
            "powershell Get-NetAdapter",
            rest,
            NETADAPTER_PIPELINE,
            filter_get_netadapter,
        ),
        "Get-NetTCPConnection" => run_table_cmdlet(
            "Get-NetTCPConnection",
            "powershell Get-NetTCPConnection",
            rest,
            NETTCPCONNECTION_PIPELINE,
            filter_get_nettcpconnection,
        ),
        // Get-SystemInfo is not a real PowerShell cmdlet (that's legacy
        // systeminfo.exe); rtk aliases it to Get-ComputerInfo, the modern
        // equivalent, purely for convenience.
        "Get-ComputerInfo" | "Get-SystemInfo" => run_plain_cmdlet(
            "Get-ComputerInfo",
            "powershell Get-ComputerInfo",
            rest,
            filter_get_computerinfo,
        ),
        "Get-Package" => run_table_cmdlet(
            "Get-Package",
            "powershell Get-Package",
            rest,
            PACKAGE_PIPELINE,
            filter_get_package,
        ),
        "Install-Package" => run_plain_cmdlet(
            "Install-Package",
            "powershell Install-Package",
            rest,
            filter_install_package,
        ),
        "Get-EventLog" => run_table_cmdlet(
            "Get-EventLog",
            "powershell Get-EventLog",
            rest,
            EVENTLOG_PIPELINE,
            filter_get_eventlog,
        ),
        "Get-Counter" => run_plain_cmdlet(
            "Get-Counter",
            "powershell Get-Counter",
            rest,
            filter_get_counter,
        ),

        // Minimal-output cmdlets: PowerShell already prints little to nothing
        // for these (a single confirmation line, or nothing at all on
        // success), so RTK passes them through unfiltered rather than
        // fabricate a "filter" with nothing meaningful to condense. Still
        // tracked (0% savings), the same way `rtk proxy` is.
        "Copy-Item" | "cp" | "copy" | "Move-Item" | "mi" | "mv" | "move" | "New-Item" | "ni"
        | "Set-Location" | "sl" | "cd" | "Get-Location" | "gl" | "pwd" | "Start-Process"
        | "saps" | "start" | "Stop-Process" | "kill" | "spps" | "Test-Path" | "test"
        | "Remove-Item" | "ri" | "rm" | "del" | "erase" | "Join-Path" | "Write-Output"
        | "write" | "echo" | "Out-String" | "Set-ItemProperty" | "sp" | "Clear-EventLog"
        | "Sort-Object" | "sort" | "Format-Table" | "ft" => run_passthrough(cmdlet, rest),

        // Unknown/unsupported cmdlet: passthrough rather than error out — RTK
        // must never block a command it doesn't specifically recognize. The
        // cmdlet name is untrusted here, so `run_passthrough` allowlists it
        // (see `validate_cmdlet_name`) before it can reach a script.
        _ => run_passthrough(cmdlet, rest),
    }
}

// ---------------------------------------------------------------------------
// Command construction & execution
// ---------------------------------------------------------------------------

/// Reject any cmdlet name that doesn't match [`VALID_CMDLET_NAME`] before it
/// can reach a `-Command` script. This is the sole gate for the
/// unknown-cmdlet passthrough path; known dispatch-table cmdlet names are
/// rtk-controlled literals and never need it, but calling this
/// unconditionally from [`run_passthrough`] makes "the cmdlet name is always
/// allowlisted before use" a property of the function, not of its callers.
fn validate_cmdlet_name(name: &str) -> Result<()> {
    if VALID_CMDLET_NAME.is_match(name) {
        Ok(())
    } else {
        anyhow::bail!("rtk powershell: invalid cmdlet name '{}'", name)
    }
}

/// Resolve which PowerShell binary to invoke. Prefers `pwsh` (PowerShell 7+,
/// cross-platform — available on Windows/macOS/Linux) when it's on `PATH`;
/// falls back to Windows PowerShell 5.1's `powershell.exe` on Windows, or
/// `pwsh` elsewhere (the call fails with a clear "binary not found" error if
/// neither is installed on macOS/Linux, rather than silently doing nothing).
fn powershell_binary() -> &'static str {
    if tool_exists("pwsh") {
        "pwsh"
    } else if cfg!(windows) {
        "powershell.exe"
    } else {
        "pwsh"
    }
}

/// Name of the environment variable used to pass `args[index]` to the child
/// PowerShell process. See [`build_powershell_script`].
fn env_var_name(index: usize) -> String {
    format!("RTK_PS_ARG_{index}")
}

/// Detect whether the user already supplied their own pipeline stage, so RTK
/// doesn't append a redundant `Select-Object | Format-Table` after it.
///
/// This only ever compares the literal argument VALUES against a fixed set
/// of known stage names — it decides which literal pipeline suffix rtk
/// itself appends, and never influences how those argument values reach the
/// script (see [`build_powershell_script`]), so it carries no injection
/// risk regardless of what the argument values contain.
fn has_user_pipeline(args: &[String]) -> bool {
    args.iter().any(|a| {
        matches!(
            a.as_str(),
            "|" | "Format-Table"
                | "Format-List"
                | "Select-Object"
                | "Where-Object"
                | "Sort-Object"
                | "Group-Object"
                | "Out-String"
                | "ConvertTo-Json"
        )
    })
}

/// Build the PowerShell script string: `<Cmdlet> <arg-or-placeholder...> [| <pipeline>]`.
///
/// No argument VALUE is ever interpolated into this string. Each positional
/// slot is either:
/// - embedded literally, when it matches [`SAFE_FLAG_TOKEN`] (a bare
///   `-Name`-shaped switch/parameter token with no special characters), so
///   it's parsed as a named parameter rather than a positional string, or
/// - replaced with `$env:RTK_PS_ARG_<i>`, referencing the environment
///   variable [`powershell_command`] sets for that argument — PowerShell
///   expands this to the variable's raw string content with no further
///   parsing, so arbitrary content (quotes — including Unicode smart quotes,
///   `$(...)`, backticks, `;`, `#`, ...) can never be interpreted as script
///   syntax.
///
/// `raw_args` is also used to decide whether to append `pipeline` (see
/// [`has_user_pipeline`]).
fn build_powershell_script(cmdlet: &str, raw_args: &[String], pipeline: Option<&str>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(raw_args.len() + 1);
    parts.push(cmdlet.to_string());
    for (i, arg) in raw_args.iter().enumerate() {
        if SAFE_FLAG_TOKEN.is_match(arg) {
            parts.push(arg.clone());
        } else {
            parts.push(format!("$env:{}", env_var_name(i)));
        }
    }
    let mut command = parts.join(" ");

    if let Some(p) = pipeline {
        if !has_user_pipeline(raw_args) {
            command.push_str(" | ");
            command.push_str(p);
        }
    }

    command
}

/// Build a ready-to-run [`Command`] for `<cmdlet> <args>`, wrapped in
/// `powershell(.exe)`/`pwsh -NoProfile -NonInteractive -Command <script>`.
/// Every argument value is set as an environment variable on the child
/// process; see [`build_powershell_script`] for how (and whether) the script
/// references it.
fn powershell_command(cmdlet: &str, args: &[String], pipeline: Option<&str>) -> Command {
    let script = build_powershell_script(cmdlet, args, pipeline);
    let mut cmd = resolved_command(powershell_binary());
    for (i, arg) in args.iter().enumerate() {
        cmd.env(env_var_name(i), arg);
    }
    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(script);
    cmd
}

/// Run a table-formatted cmdlet through the shared filter runner. `pipeline`
/// is appended (unless the user supplied their own) to project the cmdlet's
/// output down to the columns the matching filter expects.
fn run_table_cmdlet(
    ps_name: &str,
    label: &str,
    args: &[String],
    pipeline: &str,
    filter_fn: fn(&str) -> String,
) -> Result<i32> {
    let cmd = powershell_command(ps_name, args, Some(pipeline));
    runner::run_filtered(
        cmd,
        label,
        &args.join(" "),
        filter_fn,
        RunOptions::default().early_exit_on_failure(),
    )
}

/// Run a cmdlet through the shared filter runner without adding a pipeline
/// suffix — for cmdlets whose default formatting is already what the filter
/// expects (list-style single objects, free-form text, match lists, ...).
fn run_plain_cmdlet(
    ps_name: &str,
    label: &str,
    args: &[String],
    filter_fn: fn(&str) -> String,
) -> Result<i32> {
    let cmd = powershell_command(ps_name, args, None);
    runner::run_filtered(
        cmd,
        label,
        &args.join(" "),
        filter_fn,
        RunOptions::default().early_exit_on_failure(),
    )
}

/// Run a cmdlet with no filtering at all — stdin/stdout/stderr inherited
/// directly, like `rtk proxy`. Used for cmdlets whose output is already
/// minimal or that are interactive/side-effecting, and as the fallback for
/// any cmdlet RTK doesn't specifically recognize.
///
/// `cmdlet` is allowlisted via [`validate_cmdlet_name`] before it can reach a
/// script — the only caller that can pass untrusted input here is the
/// unknown-cmdlet fallback in [`run`]; every other caller passes an
/// rtk-controlled literal, which trivially satisfies the allowlist.
fn run_passthrough(cmdlet: &str, args: &[String]) -> Result<i32> {
    validate_cmdlet_name(cmdlet)?;
    let cmd = powershell_command(cmdlet, args, None);
    let label = if args.is_empty() {
        cmdlet.to_string()
    } else {
        format!("{} {}", cmdlet, args.join(" "))
    };
    runner::run(
        cmd,
        "powershell",
        &label,
        RunMode::Passthrough,
        RunOptions::default(),
    )
}

// ---------------------------------------------------------------------------
// Shared table-filtering helpers
// ---------------------------------------------------------------------------

/// Non-empty, right-trimmed lines of `output`, in order.
fn non_blank_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect()
}

/// A `Format-Table` underline row (`----  -------  ------`): dashes and
/// whitespace only.
fn is_separator_row(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && t.chars().all(|c| c == '-' || c.is_whitespace())
}

/// Merge continuation lines (from `Format-Table -Wrap`, or from cmdlets like
/// `Get-Counter` whose default view always splits a record's label and value
/// onto separate lines) back into the single logical record they belong to.
///
/// `Format-Table` always starts a new record at column 0; a continuation of
/// a wrapped column (or a following value line) is padded with leading
/// whitespace. Without this, each continuation line would be miscounted as
/// its own row, corrupting the row cap and risking a cut mid-record when the
/// cap is hit.
fn group_wrapped_rows(lines: &[&str]) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for line in lines {
        let is_continuation = line.starts_with(' ') || line.starts_with('\t');
        if is_continuation {
            if let Some(last) = rows.last_mut() {
                last.push('\n');
                last.push_str(line);
                continue;
            }
        }
        rows.push((*line).to_string());
    }
    rows
}

/// Split `Format-Table`-style output into an optional header line and its
/// data rows (see [`group_wrapped_rows`] for how continuation lines are
/// folded back into their record).
///
/// The header is the first line containing every string in `column_hints`.
/// Everything before it (banners like `Directory: C:\...`, blank lines) is
/// dropped; the `----` underline row `Format-Table` emits right after the
/// header is dropped too. When no line matches every hint (e.g. a custom
/// `-Property` projection RTK doesn't anticipate, or an error message with no
/// table at all), returns `(None, lines)` with every non-blank line intact so
/// callers still cap something sensible instead of silently losing output.
fn split_table<'a>(output: &'a str, column_hints: &[&str]) -> (Option<&'a str>, Vec<String>) {
    let lines = non_blank_lines(output);
    let header_idx = lines
        .iter()
        .position(|l| column_hints.iter().all(|h| l.contains(h)));

    match header_idx {
        Some(idx) => {
            let data_lines: Vec<&str> = lines[idx + 1..]
                .iter()
                .filter(|l| !is_separator_row(l))
                .copied()
                .collect();
            (Some(lines[idx]), group_wrapped_rows(&data_lines))
        }
        None => (None, lines.iter().map(|s| s.to_string()).collect()),
    }
}

/// Cap `lines` at `max` entries, appending an overflow note (via
/// [`join_with_overflow`]) when there were more than `max`.
fn cap_lines<S: AsRef<str>>(lines: &[S], max: usize, label: &str) -> String {
    let capped: Vec<String> = lines
        .iter()
        .take(max)
        .map(|s| s.as_ref().to_string())
        .collect();
    join_with_overflow(&capped, lines.len(), max, label)
}

/// Append a tee recovery hint to `body` when `total` exceeded `max`, so a
/// `"… +N more"` overflow note always has a way back to the hidden items
/// (see `src/cmds/README.md`, "Internal Truncation Recovery").
///
/// `multiline_rows` selects which tee helper fits the data shape:
/// - `false`: [`crate::core::tee::force_tee_tail_hint`] — a `tail -n +N`
///   byte offset, valid when every row is exactly one line (the common case).
/// - `true`: [`crate::core::tee::force_tee_hint`] — a plain "full output"
///   pointer, for rows that can themselves span multiple physical lines
///   (`-Wrap` continuation groups, `Get-Counter` samples), where a line
///   offset would land in the middle of an unrelated row.
///
/// Returns `body` unchanged if tee is disabled/skipped (`RTK_TEE=0`, or the
/// user's config) — recovery is best-effort, never a hard requirement.
fn append_recovery_hint(
    mut body: String,
    full_content: &str,
    slug: &str,
    max: usize,
    total: usize,
    multiline_rows: bool,
) -> String {
    if total <= max {
        return body;
    }
    let hint = if multiline_rows {
        crate::core::tee::force_tee_hint(full_content, slug)
    } else {
        crate::core::tee::force_tee_tail_hint(full_content, slug, max + 1)
    };
    if let Some(h) = hint {
        body.push(' ');
        body.push_str(&h);
    }
    body
}

/// Combine an optional header with its (capped) data rows, appending a tee
/// recovery hint via [`append_recovery_hint`] when rows were truncated.
fn format_capped_table(
    header: Option<&str>,
    rows: Vec<String>,
    max: usize,
    label: &str,
    slug: &str,
    multiline_rows: bool,
) -> String {
    if rows.is_empty() {
        return header.unwrap_or_default().to_string();
    }
    let total = rows.len();
    let full = rows.join("\n");
    let body = cap_lines(&rows, max, label);
    let body = append_recovery_hint(body, &full, slug, max, total, multiline_rows);
    match header {
        Some(h) => format!("{}\n{}", h, body),
        None => body,
    }
}

/// Keep only the `Key : Value` lines whose key exactly matches one of `keep`,
/// for list-formatted single-object cmdlets (`Test-NetConnection`,
/// `Get-ComputerInfo`) that otherwise print dozens of low-signal properties.
/// Falls back to the trimmed raw output when nothing matched, so an
/// unexpected format (or an error message) is never silently dropped.
fn filter_keyed_properties(output: &str, keep: &[&str]) -> String {
    let kept: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let key = line.split(':').next().unwrap_or("").trim();
            keep.contains(&key)
        })
        .collect();

    if kept.is_empty() {
        output.trim().to_string()
    } else {
        kept.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Table-style cmdlet filters (header + capped data rows)
// ---------------------------------------------------------------------------

const CHILDITEM_PIPELINE: &str = "Format-Table -AutoSize";
const MAX_CHILDITEM_ROWS: usize = CAP_INVENTORY;

fn filter_get_childitem(output: &str) -> String {
    let (header, rows) = split_table(output, &["Mode", "LastWriteTime", "Length", "Name"]);
    format_capped_table(
        header,
        rows,
        MAX_CHILDITEM_ROWS,
        "items",
        "powershell-get-childitem",
        false,
    )
}

const ITEM_PIPELINE: &str =
    "Select-Object FullName,Length,LastWriteTime,Mode | Format-Table -AutoSize";
const MAX_ITEM_ROWS: usize = CAP_LIST;

fn filter_get_item(output: &str) -> String {
    let (header, rows) = split_table(output, &["FullName", "Length", "LastWriteTime"]);
    format_capped_table(
        header,
        rows,
        MAX_ITEM_ROWS,
        "items",
        "powershell-get-item",
        false,
    )
}

const PROCESS_PIPELINE: &str = "Select-Object Name,Id,CPU,WorkingSet | Format-Table -AutoSize";
const MAX_PROCESS_ROWS: usize = CAP_INVENTORY;

fn filter_get_process(output: &str) -> String {
    let (header, rows) = split_table(output, &["Name", "Id", "CPU", "WorkingSet"]);
    format_capped_table(
        header,
        rows,
        MAX_PROCESS_ROWS,
        "processes",
        "powershell-get-process",
        false,
    )
}

const SERVICE_PIPELINE: &str = "Select-Object Name,DisplayName,Status | Format-Table -AutoSize";
const MAX_SERVICE_ROWS: usize = CAP_LIST;

fn filter_get_service(output: &str) -> String {
    let (header, rows) = split_table(output, &["Name", "DisplayName", "Status"]);
    format_capped_table(
        header,
        rows,
        MAX_SERVICE_ROWS,
        "services",
        "powershell-get-service",
        false,
    )
}

const NETADAPTER_PIPELINE: &str =
    "Select-Object Name,InterfaceDescription,Status,LinkSpeed | Format-Table -AutoSize";
const MAX_NETADAPTER_ROWS: usize = CAP_LIST;

fn filter_get_netadapter(output: &str) -> String {
    let (header, rows) = split_table(output, &["Name", "InterfaceDescription", "Status"]);
    format_capped_table(
        header,
        rows,
        MAX_NETADAPTER_ROWS,
        "adapters",
        "powershell-get-netadapter",
        false,
    )
}

const NETTCPCONNECTION_PIPELINE: &str =
    "Select-Object LocalAddress,LocalPort,RemoteAddress,RemotePort,State,OwningProcess | Format-Table -AutoSize";
const MAX_NETTCPCONNECTION_ROWS: usize = CAP_INVENTORY;

fn filter_get_nettcpconnection(output: &str) -> String {
    let (header, rows) = split_table(output, &["LocalAddress", "LocalPort", "RemoteAddress"]);
    format_capped_table(
        header,
        rows,
        MAX_NETTCPCONNECTION_ROWS,
        "connections",
        "powershell-get-nettcpconnection",
        false,
    )
}

const PACKAGE_PIPELINE: &str =
    "Select-Object Name,Version,ProviderName,Source | Format-Table -AutoSize";
const MAX_PACKAGE_ROWS: usize = CAP_LIST;

fn filter_get_package(output: &str) -> String {
    let (header, rows) = split_table(output, &["Name", "Version", "ProviderName"]);
    format_capped_table(
        header,
        rows,
        MAX_PACKAGE_ROWS,
        "packages",
        "powershell-get-package",
        false,
    )
}

const WINEVENT_PIPELINE: &str =
    "Select-Object TimeCreated,Id,LevelDisplayName,ProviderName,Message | Format-Table -AutoSize -Wrap";
const MAX_WINEVENT_ROWS: usize = CAP_INVENTORY;

fn filter_get_winevent(output: &str) -> String {
    let (header, rows) = split_table(output, &["TimeCreated", "Id", "LevelDisplayName"]);
    format_capped_table(
        header,
        rows,
        MAX_WINEVENT_ROWS,
        "events",
        "powershell-get-winevent",
        true,
    )
}

const EVENTLOG_PIPELINE: &str =
    "Select-Object TimeGenerated,EntryType,Source,Message | Format-Table -AutoSize -Wrap";
const MAX_EVENTLOG_ROWS: usize = CAP_INVENTORY;

fn filter_get_eventlog(output: &str) -> String {
    let (header, rows) = split_table(output, &["TimeGenerated", "EntryType", "Source"]);
    format_capped_table(
        header,
        rows,
        MAX_EVENTLOG_ROWS,
        "log entries",
        "powershell-get-eventlog",
        true,
    )
}

// ---------------------------------------------------------------------------
// Unstructured / line-capped cmdlet filters
// ---------------------------------------------------------------------------

const MAX_SELECT_STRING_MATCHES: usize = CAP_INVENTORY;

fn filter_select_string(output: &str) -> String {
    let lines = non_blank_lines(output);
    let total = lines.len();
    let full = lines.join("\n");
    let body = cap_lines(&lines, MAX_SELECT_STRING_MATCHES, "matches");
    append_recovery_hint(
        body,
        &full,
        "powershell-select-string",
        MAX_SELECT_STRING_MATCHES,
        total,
        false,
    )
}

const MAX_COUNTER_SAMPLES: usize = CAP_INVENTORY;

/// `Get-Counter`'s default view always splits a sample onto two physical
/// lines (the counter path, then an indented value line) — every sample is
/// one logical row, so [`group_wrapped_rows`] folds the value line back in
/// before capping. Without this, capping would count each sample as two
/// rows and could show a counter path with its value cut off. Each kept
/// sample can still span two lines, so the recovery hint uses the
/// multiline-safe `force_tee_hint` rather than a `tail -n +N` line offset.
fn filter_get_counter(output: &str) -> String {
    let lines = non_blank_lines(output);
    let rows = group_wrapped_rows(&lines);
    let total = rows.len();
    let full = rows.join("\n");
    let body = cap_lines(&rows, MAX_COUNTER_SAMPLES, "counter samples");
    append_recovery_hint(
        body,
        &full,
        "powershell-get-counter",
        MAX_COUNTER_SAMPLES,
        total,
        true,
    )
}

/// `Get-Content` prints a file verbatim; RTK only steps in for large files,
/// keeping a head/tail window (like `rtk read --tail-lines`) instead of the
/// language-aware filtering `rtk read` applies, since the content here is
/// arbitrary (not necessarily source code).
const CONTENT_LINE_THRESHOLD: usize = 500;
const CONTENT_EDGE_LINES: usize = 20;

fn filter_get_content(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= CONTENT_LINE_THRESHOLD {
        return output.to_string();
    }

    let omitted = lines.len() - 2 * CONTENT_EDGE_LINES;
    let head = lines[..CONTENT_EDGE_LINES].join("\n");
    let tail = lines[lines.len() - CONTENT_EDGE_LINES..].join("\n");
    format!("{}\n... [truncated {} lines] ...\n{}", head, omitted, tail)
}

// ---------------------------------------------------------------------------
// Registry / networking / system-info filters
// ---------------------------------------------------------------------------

const MAX_ITEMPROPERTY_ROWS: usize = CAP_LIST;

/// `Get-ItemProperty`'s real default output is a flat `Key : Value` list
/// (like `Get-ComputerInfo`), not a table — a single registry key can easily
/// carry dozens of values. `PSPath`/`PSParentPath` duplicate the same (often
/// long) registry path twice and add no information beyond what the
/// command's own arguments already say, so they're dropped. The remaining
/// PS* metadata properties commonly print *after* every real value (PowerShell
/// appends them), so `PSChildName` — the one that identifies which key these
/// properties belong to — is pulled to the front and kept unconditionally,
/// rather than risk it falling past the cap on a key with many values.
fn filter_get_itemproperty(output: &str) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let key = l.split(':').next().unwrap_or("").trim();
            key != "PSPath" && key != "PSParentPath"
        })
        .collect();

    if lines.is_empty() {
        return String::new();
    }

    let key_of = |l: &&str| l.split(':').next().unwrap_or("").trim().to_string();
    let child_name = lines.iter().find(|l| key_of(l) == "PSChildName").copied();
    let rest: Vec<&str> = lines
        .iter()
        .filter(|l| key_of(l) != "PSChildName")
        .copied()
        .collect();

    let total = rest.len();
    let full = rest.join("\n");
    let capped = cap_lines(&rest, MAX_ITEMPROPERTY_ROWS, "properties");
    let capped = append_recovery_hint(
        capped,
        &full,
        "powershell-get-itemproperty",
        MAX_ITEMPROPERTY_ROWS,
        total,
        false,
    );
    match child_name {
        Some(name) => format!("{}\n{}", name, capped),
        None => capped,
    }
}

const NET_CONNECTION_KEEP: &[&str] = &[
    "ComputerName",
    "RemoteAddress",
    "PingSucceeded",
    "TcpTestSucceeded",
];

fn filter_test_netconnection(output: &str) -> String {
    filter_keyed_properties(output, NET_CONNECTION_KEEP)
}

const COMPUTER_INFO_KEEP: &[&str] = &["CsName", "CsDomain", "OsName", "OsVersion", "OsArchitecture"];

fn filter_get_computerinfo(output: &str) -> String {
    filter_keyed_properties(output, COMPUTER_INFO_KEEP)
}

const INSTALL_PACKAGE_KEYWORDS: &[&str] = &["status", "completed", "installed", "failed", "error"];

/// `Install-Package` output ranges from silent to a wall of dependency
/// resolution / download progress noise; keep only lines that look like a
/// status or outcome, matching case-insensitively.
fn filter_install_package(output: &str) -> String {
    let kept: Vec<&str> = output
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            INSTALL_PACKAGE_KEYWORDS.iter().any(|k| lower.contains(k))
        })
        .collect();

    if kept.is_empty() {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            "ok".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        kept.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    fn savings_pct(raw: &str, filtered: &str) -> f64 {
        let raw_tokens = count_tokens(raw);
        if raw_tokens == 0 {
            return 0.0;
        }
        100.0 - (count_tokens(filtered) as f64 / raw_tokens as f64 * 100.0)
    }

    /// Fixture files under `tests/fixtures/powershell/` begin with a
    /// `# rtk fixture note: ...` provenance comment documenting how the
    /// fixture was captured/anonymized (see `src/cmds/powershell/README.md`).
    /// Real PowerShell output never contains such a line, so tests strip it
    /// before treating the fixture as "raw command output" -- both for
    /// filter input and for the token-savings baseline, so the comment can't
    /// skew either.
    fn strip_fixture_note(input: &str) -> String {
        let mut lines = input.lines();
        match lines.next() {
            Some(first) if first.trim_start().starts_with('#') => {
                format!("{}\n", lines.collect::<Vec<_>>().join("\n"))
            }
            _ => input.to_string(),
        }
    }

    // ---- vuln #1 regression: cmdlet-name allowlist ----

    #[test]
    fn test_validate_cmdlet_name_accepts_real_shapes() {
        for name in ["Get-Process", "gci", "ls", "A1-B2", "dir", "Get-SystemInfo"] {
            assert!(
                validate_cmdlet_name(name).is_ok(),
                "expected '{}' to be accepted",
                name
            );
        }
    }

    #[test]
    fn test_validate_cmdlet_name_rejects_injection_payload() {
        // Confirmed PoC: this string as the "cmdlet" argument previously
        // reached the script unquoted and executed `whoami` as a second
        // statement.
        assert!(validate_cmdlet_name("Get-Date; whoami; #").is_err());
    }

    #[test]
    fn test_validate_cmdlet_name_rejects_malformed_shapes() {
        for name in [
            "",
            " ",
            "1cmdlet",
            "-cmdlet",
            "Get Process",
            "Get-Date;whoami",
            "a$b",
            "a`b",
            "a|b",
            "a#b",
        ] {
            assert!(
                validate_cmdlet_name(name).is_err(),
                "expected '{}' to be rejected",
                name
            );
        }
    }

    #[test]
    fn test_run_rejects_injection_payload_before_spawning_powershell() {
        // Exercises the real code path: an unrecognized "cmdlet" argument
        // falls through to `run_passthrough` in `run()`. Must fail validation
        // (and therefore never spawn a process) regardless of whether
        // PowerShell is installed on the machine running this test.
        let result = run(&["Get-Date; whoami; #".to_string()], 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_rejects_missing_cmdlet_gracefully() {
        let result = run(&[], 0);
        assert!(result.is_err());
    }

    // ---- vuln #2 regression: argument values never reach the script text ----

    #[test]
    fn test_powershell_command_never_embeds_argument_values_in_script() {
        // Confirmed PoC payloads: a Unicode smart-quote variant terminates a
        // naive `'...'`-wrapped script string early, letting `;whoami;#`
        // execute. None of these start with `-`, so all route through the
        // env-var channel (see `build_powershell_script`).
        let payloads = [
            "x\u{2019};whoami;#", // U+2019 RIGHT SINGLE QUOTATION MARK
            "x\u{2018};whoami;#", // U+2018 LEFT SINGLE QUOTATION MARK
            "x\u{201A};whoami;#", // U+201A SINGLE LOW-9 QUOTATION MARK
            "x\u{201B};whoami;#", // U+201B SINGLE HIGH-REVERSED-9 QUOTATION MARK
            "'; whoami; #",       // ASCII single quote
            "$(whoami)",
            "`whoami`",
        ];

        for payload in payloads {
            let args = vec![payload.to_string()];
            let cmd = powershell_command("Write-Output", &args, None);

            let script_arg = cmd
                .get_args()
                .last()
                .and_then(|a| a.to_str())
                .expect("expected a -Command script argument");
            assert!(
                script_arg.contains("$env:RTK_PS_ARG_0"),
                "script should reference the env placeholder for payload {:?}, got: {}",
                payload,
                script_arg
            );
            assert!(
                !script_arg.contains("whoami"),
                "payload leaked into script text: {:?} -> {}",
                payload,
                script_arg
            );

            let env_value = cmd
                .get_envs()
                .find(|(k, _)| *k == "RTK_PS_ARG_0")
                .and_then(|(_, v)| v)
                .and_then(|v| v.to_str());
            assert_eq!(
                env_value,
                Some(payload),
                "payload should be reachable only via the env var, unmodified"
            );
        }
    }

    #[test]
    fn test_build_powershell_script_never_embeds_arg_values() {
        let args = vec![
            "C:\\Program Files".to_string(),
            "Get-Date; whoami; #".to_string(),
        ];
        let script = build_powershell_script("Test-Path", &args, None);
        assert_eq!(script, "Test-Path $env:RTK_PS_ARG_0 $env:RTK_PS_ARG_1");
        assert!(!script.contains("whoami"));
    }

    #[test]
    fn test_build_powershell_script_embeds_safe_flags_literally() {
        let args = vec!["-Recurse".to_string()];
        let script = build_powershell_script("Get-ChildItem", &args, None);
        assert_eq!(script, "Get-ChildItem -Recurse");
    }

    #[test]
    fn test_build_powershell_script_flag_shaped_injection_still_routes_through_env() {
        // Starts with `-` but isn't a bare flag token (contains `;`/`#`), so
        // it must NOT be embedded literally.
        let args = vec!["-Recurse;whoami;#".to_string()];
        let script = build_powershell_script("Get-Process", &args, None);
        assert_eq!(script, "Get-Process $env:RTK_PS_ARG_0");
        assert!(!script.contains("whoami"));
    }

    #[test]
    fn test_build_powershell_script_mixes_flags_and_values() {
        let args = vec![
            "-Path".to_string(),
            "C:\\temp".to_string(),
            "-Recurse".to_string(),
        ];
        let script = build_powershell_script("Get-ChildItem", &args, None);
        assert_eq!(
            script,
            "Get-ChildItem -Path $env:RTK_PS_ARG_1 -Recurse"
        );
    }

    #[test]
    fn test_build_powershell_script_zero_args() {
        assert_eq!(build_powershell_script("Get-Process", &[], None), "Get-Process");
    }

    #[test]
    fn test_build_powershell_script_appends_pipeline_by_default() {
        let args = vec!["-Recurse".to_string()];
        let script = build_powershell_script("Get-ChildItem", &args, Some("Format-Table"));
        assert_eq!(script, "Get-ChildItem -Recurse | Format-Table");
    }

    #[test]
    fn test_build_powershell_script_skips_pipeline_if_user_supplied_one() {
        let args = vec!["Format-Table".to_string()];
        let script = build_powershell_script("Get-Process", &args, Some("Select-Object Name"));
        assert_eq!(script, "Get-Process $env:RTK_PS_ARG_0");
    }

    #[test]
    fn test_has_user_pipeline_detects_known_stages() {
        assert!(has_user_pipeline(&["Format-Table".to_string()]));
        assert!(has_user_pipeline(&["Where-Object".to_string()]));
        assert!(!has_user_pipeline(&["-Recurse".to_string()]));
    }

    #[test]
    fn test_powershell_binary_returns_known_binary() {
        let bin = powershell_binary();
        assert!(bin == "pwsh" || bin == "powershell.exe");
    }

    // ---- shared table helpers ----

    #[test]
    fn test_is_separator_row() {
        assert!(is_separator_row("----  ------  ----"));
        assert!(!is_separator_row("chrome 1234"));
        assert!(!is_separator_row(""));
    }

    #[test]
    fn test_cap_lines_generic_over_str_and_string() {
        let borrowed: Vec<&str> = vec!["a", "b", "c"];
        assert_eq!(cap_lines(&borrowed, 2, "x"), "a\nb\n\u{2026} +1 more x");

        let owned: Vec<String> = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(cap_lines(&owned, 2, "x"), "a\nb\n\u{2026} +1 more x");
    }

    #[test]
    fn test_split_table_drops_banner_and_separator() {
        let input = "\n    Directory: C:\\temp\n\nMode  LastWriteTime  Length  Name\n----  -------------  ------  ----\nd----- 4/1/2026 file1\n-a---- 4/1/2026 file2\n";
        let (header, rows) = split_table(input, &["Mode", "LastWriteTime", "Length", "Name"]);
        assert_eq!(header, Some("Mode  LastWriteTime  Length  Name"));
        assert_eq!(
            rows,
            vec![
                "d----- 4/1/2026 file1".to_string(),
                "-a---- 4/1/2026 file2".to_string()
            ]
        );
    }

    #[test]
    fn test_split_table_falls_back_when_no_header_matches() {
        let input = "Get-Process : Cannot find a process with the name \"nope\".\n";
        let (header, rows) = split_table(input, &["Name", "Id", "CPU", "WorkingSet"]);
        assert_eq!(header, None);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn test_split_table_merges_wrap_continuation_lines_into_one_row() {
        // Reproduces the real `Format-Table -Wrap` shape: a continuation line
        // is padded with leading whitespace and belongs to the PRECEDING
        // record, not a new one.
        let input = "\
TimeCreated           Id LevelDisplayName Message
-----------           -- ---------------- -------
4/20/2026 10:01:00 AM 1001 Warning        Sample event message that wraps
                                           onto a continuation line
4/20/2026 10:02:00 AM 1002 Error          Second event, single line
";
        let (header, rows) = split_table(input, &["TimeCreated", "Id", "LevelDisplayName"]);
        assert!(header.is_some());
        // Two logical records, not three raw lines.
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains("wraps"));
        assert!(rows[0].contains("onto a continuation line"));
        assert!(rows[1].contains("Second event"));
    }

    #[test]
    fn test_group_wrapped_rows_continuation_at_start_falls_back_to_new_row() {
        // Defensive: a continuation-shaped line with no preceding row must
        // not panic or vanish.
        let rows = group_wrapped_rows(&["  leading whitespace, no prior row"]);
        assert_eq!(rows, vec!["  leading whitespace, no prior row".to_string()]);
    }

    #[test]
    fn test_format_capped_table_appends_overflow_note() {
        // RTK_TEE=0 keeps this deterministic: with recovery disabled,
        // `append_recovery_hint` is a no-op and the overflow note is exact.
        std::env::set_var("RTK_TEE", "0");
        let rows = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let out = format_capped_table(Some("H"), rows, 2, "items", "test-slug", false);
        std::env::remove_var("RTK_TEE");
        assert_eq!(out, "H\na\nb\n\u{2026} +1 more items");
    }

    #[test]
    fn test_format_capped_table_empty_rows_returns_header_only() {
        let out = format_capped_table(Some("H"), vec![], 10, "items", "test-slug", false);
        assert_eq!(out, "H");
    }

    #[test]
    fn test_append_recovery_hint_noop_when_not_truncated() {
        let body = "a\nb".to_string();
        let out = append_recovery_hint(body.clone(), "a\nb", "test-slug", 5, 2, false);
        assert_eq!(out, body);
    }

    #[test]
    fn test_append_recovery_hint_respects_rtk_tee_disable() {
        // Both the tail and block variants must degrade to a no-op (never
        // panic, never fabricate a hint) when recovery is disabled.
        std::env::set_var("RTK_TEE", "0");
        let tail = append_recovery_hint("a\nb".to_string(), "a\nb\nc", "test-slug", 2, 3, false);
        let block = append_recovery_hint("a\nb".to_string(), "a\nb\nc", "test-slug", 2, 3, true);
        std::env::remove_var("RTK_TEE");
        assert_eq!(tail, "a\nb");
        assert_eq!(block, "a\nb");
    }

    #[test]
    fn test_filter_keyed_properties_exact_key_match() {
        let input = "ComputerName : example.com\nCsDomainRole : StandaloneWorkstation\n";
        // "CsDomain" must NOT match "CsDomainRole" — exact key match only.
        let out = filter_keyed_properties(input, &["ComputerName", "CsDomain"]);
        assert_eq!(out, "ComputerName : example.com");
    }

    #[test]
    fn test_filter_keyed_properties_falls_back_when_nothing_matches() {
        let input = "SomeOtherProp : value\n";
        let out = filter_keyed_properties(input, &["ComputerName"]);
        assert_eq!(out, "SomeOtherProp : value");
    }

    // ---- per-cmdlet filters: fixtures + token savings ----

    #[test]
    fn test_filter_get_childitem() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_childitem_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_childitem(&input);

        assert!(output.contains("Mode"));
        // `non_blank_lines` right-trims every row, so a kept row's last
        // column has no trailing padding left to assert on — match on the
        // unambiguous leading part of the row instead.
        assert!(output.contains("docs"));
        assert!(!output.contains("Directory:"));
        assert!(output.contains("more items"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-ChildItem: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_process() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_process_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_process(&input);

        assert!(output.contains("Name"));
        assert!(output.contains("ExampleAgent"));
        assert!(output.contains("more processes"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Process: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_service() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_service_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_service(&input);

        assert!(output.contains("DisplayName"));
        assert!(output.contains("ExampleSvc1"));
        assert!(output.contains("more services"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Service: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_item() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_item_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_item(&input);

        assert!(output.contains("FullName"));
        assert!(output.contains("report-1.txt"));
        assert!(output.contains("more items"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Item: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_netadapter() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_netadapter_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_netadapter(&input);

        assert!(output.contains("InterfaceDescription"));
        assert!(output.contains("ExampleAdapter1"));
        assert!(output.contains("more adapters"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-NetAdapter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_nettcpconnection() {
        let raw =
            include_str!("../../../tests/fixtures/powershell/get_nettcpconnection_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_nettcpconnection(&input);

        assert!(output.contains("LocalAddress"));
        assert!(output.contains("more connections"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-NetTCPConnection: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_package() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_package_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_package(&input);

        assert!(output.contains("ProviderName"));
        assert!(output.contains("Example Package 1 "));
        assert!(output.contains("more packages"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Package: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_winevent_groups_wrapped_continuations() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_winevent_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_winevent(&input);

        assert!(output.contains("LevelDisplayName"));
        assert!(output.contains("more events"));
        // A wrapped continuation line must stay attached to its record, not
        // be counted (or truncated) as a separate row.
        assert!(output.contains("continuation line one for event 3"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-WinEvent: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_eventlog_groups_wrapped_continuations() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_eventlog_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_eventlog(&input);

        assert!(output.contains("EntryType"));
        assert!(output.contains("more log entries"));
        assert!(output.contains("continuation line one for entry 3"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-EventLog: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_select_string() {
        let raw = include_str!("../../../tests/fixtures/powershell/select_string_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_select_string(&input);

        assert!(output.contains("sample doc"));
        assert!(output.contains("more matches"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Select-String: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_counter_groups_sample_value_lines() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_counter_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_counter(&input);

        assert!(output.contains("processor time"));
        assert!(output.contains("more counter samples"));
        // Each two-physical-line sample must count (and cap) as ONE row, so
        // a kept sample's value must never be separated from its label.
        for line in output.lines() {
            if line.contains("processor time :") {
                assert!(
                    line.contains('\n') || output.contains(&format!("{}\n", line.trim_end())),
                    "sample line should be followed by its value"
                );
            }
        }

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Counter: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_content_truncates_large_files() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_content_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_content(&input);

        assert!(output.contains("Application Log Start"));
        assert!(output.contains("Application Log End"));
        assert!(output.contains("... [truncated"));
        assert!(output.lines().count() < input.lines().count());

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-Content: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_content_small_file_untouched() {
        let input = "line1\nline2\nline3\n";
        assert_eq!(filter_get_content(input), input);
    }

    #[test]
    fn test_filter_get_itemproperty() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_itemproperty_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_itemproperty(&input);

        assert!(output.contains("PSChildName"));
        assert!(!output.contains("PSPath"));
        assert!(!output.contains("PSParentPath"));
        assert!(output.contains("ExampleValue1"));
        assert!(output.contains("more properties"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-ItemProperty: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_test_netconnection() {
        let raw = include_str!("../../../tests/fixtures/powershell/test_netconnection_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_test_netconnection(&input);

        assert!(output.contains("ComputerName"));
        assert!(output.contains("PingSucceeded"));
        assert!(!output.contains("NameResolutionResults"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Test-NetConnection: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_get_computerinfo() {
        let raw = include_str!("../../../tests/fixtures/powershell/get_computerinfo_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_get_computerinfo(&input);

        assert!(output.contains("CsName"));
        assert!(output.contains("OsName"));
        assert!(output.contains("OsVersion"));
        assert!(!output.contains("CsManufacturer"));
        assert!(!output.contains("BiosVersion"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Get-ComputerInfo: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_install_package() {
        let raw = include_str!("../../../tests/fixtures/powershell/install_package_raw.txt");
        let input = strip_fixture_note(raw);
        let output = filter_install_package(&input);

        assert!(output.to_ascii_lowercase().contains("installed"));
        assert!(!output.contains("Resolving dependency graph"));

        let savings = savings_pct(&input, &output);
        assert!(
            savings >= 60.0,
            "Install-Package: expected >=60% savings, got {:.1}%",
            savings
        );
    }

    // ---- robustness: empty / malformed input never panics ----

    #[test]
    fn test_filters_handle_empty_input_without_panicking() {
        assert_eq!(filter_get_childitem(""), "");
        assert_eq!(filter_get_process(""), "");
        assert_eq!(filter_select_string(""), "");
        assert_eq!(filter_get_content(""), "");
        assert_eq!(filter_get_itemproperty(""), "");
        assert_eq!(filter_test_netconnection(""), "");
        assert_eq!(filter_install_package(""), "ok");
    }

    #[test]
    fn test_filter_get_process_malformed_input_falls_back() {
        let malformed = "not valid powershell output at all";
        let output = filter_get_process(malformed);
        assert!(!output.is_empty());
        assert!(output.contains("not valid powershell output"));
    }
}

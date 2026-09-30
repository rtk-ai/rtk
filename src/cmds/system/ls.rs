//! Filters directory listings into a compact tree format.

use super::constants::NOISE_DIRS;
use crate::core::arg_tokenizer::{self, Dialect, LongOptions, Token, TokenKind, ValueSpec};
use crate::core::child_command::{OperandGrammar, OperandSplit, PathOperands, SplitArgv};
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

/// Matches the date+time portion in `ls -la` output, which serves as a
/// stable anchor regardless of owner/group column width.
/// E.g.: " Mar 31 16:18 " or " Dec 25  2024 "
static LS_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\s+(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\s+\d{1,2}\s+(?:\d{4}|\d{2}:\d{2})\s+"
    )
    .unwrap()
});

/// Which ls runs. GNU coreutils' reads `-I`, `-T` and `-w` as taking a value and `-D` as
/// `--dired`; the BSD ls of macOS and the BSDs reads `-I`, `-T` and `-w` as booleans, and
/// `-D format` is its one short flag with a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LsFlavor {
    Gnu,
    Bsd,
}

/// Whether the platform's own ls is BSD's.
const BSD_PLATFORM: bool = cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd"
));

impl LsFlavor {
    /// The flavor of the ls at `program`, the path rtk resolved, without running it: BSD only
    /// when the platform ships BSD's ls and that is the one found (see [`is_system_ls`]). Any
    /// other ls there, such as Homebrew's GNU coreutils first on `PATH`, is GNU, as is every ls
    /// elsewhere, Windows included, where it is Git for Windows' or MSYS2's coreutils. The
    /// path is canonicalized only on a BSD platform, so nowhere else pays the syscall.
    fn of(bsd_platform: bool, program: &Path) -> Self {
        if bsd_platform && is_system_ls(&canonical_program(program)) {
            LsFlavor::Bsd
        } else {
            LsFlavor::Gnu
        }
    }

    /// Every long option this ls accepts. A unique prefix of one is read as that option, as
    /// `getopt_long` reads it (see [`arg_tokenizer::resolve_long`]); a bare flag that is
    /// neither an option nor such a prefix leaves the operands unbounded.
    fn long_options(self) -> &'static LongOptions<'static> {
        match self {
            LsFlavor::Gnu => &GNU_LS_LONG_OPTIONS,
            LsFlavor::Bsd => &BSD_LS_LONG_OPTIONS,
        }
    }

    /// A long flag's full name, as this ls resolves it.
    fn long_name(self, name: &str) -> Option<&'static str> {
        arg_tokenizer::resolve_long(name, self.long_options())
    }
}

/// Whether `canonical`, a canonicalized ls path, is the BSD platform's own `/bin/ls`.
fn is_system_ls(canonical: &Path) -> bool {
    canonical == Path::new("/bin/ls")
}

/// `program`, the path rtk resolved for ls, with every symlink and `..` resolved (so a
/// `/usr/local/bin/ls` linked to `/bin/ls` is the system ls), or as it is when that fails.
fn canonical_program(program: &Path) -> Cow<'_, Path> {
    std::fs::canonicalize(program).map_or(Cow::Borrowed(program), Cow::Owned)
}

/// GNU ls (`ls --help`, coreutils 9.10).
const GNU_LS_LONG_OPTIONS: LongOptions<'static> = LongOptions::new(&[
    "all",
    "almost-all",
    "author",
    "block-size",
    "classify",
    "color",
    "context",
    "dereference",
    "dereference-command-line",
    "dereference-command-line-symlink-to-dir",
    "directory",
    "dired",
    "escape",
    "file-type",
    "format",
    "full-time",
    "group-directories-first",
    "help",
    "hide",
    "hide-control-chars",
    "human-readable",
    "hyperlink",
    "ignore",
    "ignore-backups",
    "indicator-style",
    "inode",
    "kibibytes",
    "literal",
    "no-group",
    "numeric-uid-gid",
    "quote-name",
    "quoting-style",
    "recursive",
    "reverse",
    "show-control-chars",
    "si",
    "size",
    "sort",
    "tabsize",
    "time",
    "time-style",
    "version",
    "width",
    "zero",
]);

/// BSD ls: FreeBSD's `bin/ls/ls.c` `long_opts`; Apple's `file_cmds` ls has `--color` only. Plus
/// `--all`, which BSD ls does not have but rtk takes itself on every platform: it becomes the
/// `-a` of rtk's own `-la` and is never forwarded.
const BSD_LS_LONG_OPTIONS: LongOptions<'static> = LongOptions::new(&[
    "all",
    "color",
    "group-directories",
    "group-directories-first",
]);

/// ls's value-taking flags, for the ls that runs. GNU's come from `ls --help`; `--color`,
/// `--classify` and `--hyperlink` take an optional value, which GNU ls only reads when attached:
/// `ls --color never` lists `never`. BSD's come from the `getopt_long` string in FreeBSD's and
/// Apple's `ls.c`, where only `D` takes a value, and `--color`/`--group-directories` an optional
/// one. A long flag is read under the full name it abbreviates.
fn ls_takes_value(flavor: LsFlavor, kind: TokenKind, name: &str) -> Option<ValueSpec> {
    // A required value is whatever argument comes next, a literal `--` included, as
    // `getopt` reads it: `ls -I -- -lh` ignores `--` and lists with `-lh`.
    let required = || ValueSpec::value().claiming_dash_dash();
    match (flavor, kind) {
        (LsFlavor::Gnu, TokenKind::Long) => match flavor.long_name(name)? {
            "block-size" | "format" | "hide" | "ignore" | "indicator-style" | "quoting-style"
            | "sort" | "tabsize" | "time" | "time-style" | "width" => Some(required()),
            "classify" | "color" | "hyperlink" => Some(ValueSpec::attached_only()),
            _ => None,
        },
        (LsFlavor::Gnu, TokenKind::Short) => matches!(name, "I" | "T" | "w").then(required),
        (LsFlavor::Bsd, TokenKind::Long) => {
            matches!(flavor.long_name(name), Some("color" | "group-directories"))
                .then(ValueSpec::attached_only)
        }
        (LsFlavor::Bsd, TokenKind::Short) => (name == "D").then(required),
        _ => None,
    }
}

/// True for a letter that takes a value in either flavor: GNU's `I`, `T`, `w`, BSD's `D`.
///
/// Inside a cluster, what follows such a letter is kept as written whichever ls runs: under
/// one flavor or the other it is a value, so `-Ihello` stays `-Ihello` even if the flavor were
/// guessed wrong. Whether the letter takes the *next* argument follows the flavor that runs.
fn takes_value_in_either(letter: char) -> bool {
    let mut buf = [0; 4];
    let letter: &str = letter.encode_utf8(&mut buf);
    [LsFlavor::Gnu, LsFlavor::Bsd]
        .into_iter()
        .any(|flavor| ls_takes_value(flavor, TokenKind::Short, letter).is_some())
}

fn ls_tokens<T: AsRef<str>>(flavor: LsFlavor, args: &[T]) -> Vec<Token<'_>> {
    arg_tokenizer::tokenize_grammar(
        args,
        &|kind, name| ls_takes_value(flavor, kind, name),
        Dialect::Posix,
    )
}

/// ls's grammar, for the ls that runs: its operands are the free positionals.
pub(crate) struct LsGrammar(pub(crate) LsFlavor);

/// How one argument reaches ls.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Role {
    /// A short-flag cluster, forwarded through [`trim_cluster`].
    Cluster,
    /// `--all`, which rtk passes itself.
    All,
    Operand,
    DashDash,
    /// Any other flag, or a flag's separate value, forwarded as written.
    Verbatim,
}

/// What rtk reads from ls's flags, and how each argument is forwarded.
pub(crate) struct LsArgs {
    show_all: bool,
    show_long: bool,
    wants_all: bool,
    roles: Vec<Role>,
}

impl OperandGrammar for LsGrammar {
    type Parsed = LsArgs;

    fn split<T: AsRef<str>>(&self, args: &[T]) -> (OperandSplit, LsArgs) {
        let flavor = self.0;
        let tokens = ls_tokens(flavor, args);
        let split = OperandSplit::free_positionals(&tokens, flavor.long_options());
        let mut roles = vec![Role::Verbatim; args.len()];
        for token in &tokens {
            let role = match token.kind {
                TokenKind::Short => Role::Cluster,
                TokenKind::Long
                    if token.attached.is_none() && flavor.long_name(token.text) == Some("all") =>
                {
                    Role::All
                }
                TokenKind::Positional if token.is_free_positional() => Role::Operand,
                TokenKind::DashDash => Role::DashDash,
                _ => continue,
            };
            if let Some(slot) = roles.get_mut(token.source_index) {
                *slot = role;
            }
        }
        let parsed = LsArgs {
            show_all: has_short(&tokens, &['a', 'A'])
                || has_long(&tokens, flavor, &["all", "almost-all"]),
            show_long: shows_long(&tokens, flavor),
            wants_all: has_short(&tokens, &['a']) || has_long(&tokens, flavor, &["all"]),
            roles,
        };
        (split, parsed)
    }
}

fn has_short(tokens: &[Token<'_>], letters: &[char]) -> bool {
    tokens.iter().any(|t| {
        t.kind == TokenKind::Short && t.text.chars().next().is_some_and(|c| letters.contains(&c))
    })
}

fn has_long(tokens: &[Token<'_>], flavor: LsFlavor, names: &[&str]) -> bool {
    tokens.iter().any(|t| {
        t.kind == TokenKind::Long && flavor.long_name(t.text).is_some_and(|n| names.contains(&n))
    })
}

/// Per `man ls`, the long listing is triggered by `-l` and also implied by `-g`, `-n`, `-o`,
/// `--full-time` or GNU `--format=long` and `--format=verbose`.
fn shows_long(tokens: &[Token<'_>], flavor: LsFlavor) -> bool {
    has_short(tokens, &['l', 'g', 'n', 'o'])
        || has_long(tokens, flavor, &["full-time"])
        || tokens.iter().any(|t| {
            t.kind == TokenKind::Long
                && flavor.long_name(t.text) == Some("format")
                && matches!(t.value(tokens), Some("long" | "verbose"))
        })
}

/// A short cluster as ls receives it: `l`, `a` and `h` dropped, since rtk asks for the long
/// listing itself and formats sizes, but only before the first letter that
/// [`takes_value_in_either`] flavor; `None` when nothing is left.
fn trim_cluster(cluster: &str) -> Option<String> {
    let letters = cluster.strip_prefix('-').unwrap_or(cluster);
    let mut trimmed = String::from("-");
    for (at, letter) in letters.char_indices() {
        if takes_value_in_either(letter) {
            trimmed.push_str(&letters[at..]);
            break;
        }
        if !matches!(letter, 'l' | 'a' | 'h') {
            trimmed.push(letter);
        }
    }
    (trimmed.len() > 1).then_some(trimmed)
}

/// Each argument ls receives, with its role, in the original order: clusters through
/// [`trim_cluster`], `--all` dropped, everything else as written.
fn forwarded<'a>(
    args: &'a [String],
    roles: &'a [Role],
) -> impl Iterator<Item = (Role, Cow<'a, str>)> + 'a {
    args.iter().zip(roles).filter_map(|(arg, &role)| {
        match role {
            Role::Cluster => trim_cluster(arg).map(Cow::Owned),
            Role::All => None,
            _ => Some(Cow::Borrowed(arg.as_str())),
        }
        .map(|text| (role, text))
    })
}

/// What ls receives after rtk's own `-l`/`-la`: the arguments passed literally, then the
/// operands it may glob. With a bounded split the flags come first, then `--` if the user wrote
/// one, then the operands, since BSD ls stops reading flags at its first operand. Otherwise a
/// flag no table knows may have taken the next argument, so everything keeps the typed order
/// and is literal.
fn ls_argv<'a>(
    args: &'a [String],
    argv: &SplitArgv,
    parsed: &'a LsArgs,
) -> (Vec<Cow<'a, str>>, Option<PathOperands>) {
    let forwarded = forwarded(args, &parsed.roles);
    if !argv.is_bounded() {
        return (forwarded.map(|(_, arg)| arg).collect(), None);
    }
    let mut literal: Vec<Cow<'a, str>> = forwarded
        .filter(|(role, _)| matches!(role, Role::Cluster | Role::Verbatim))
        .map(|(_, arg)| arg)
        .collect();
    if parsed.roles.contains(&Role::DashDash) {
        literal.push(Cow::Borrowed("--"));
    }
    (literal, Some(argv.operands()))
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("ls");
    let flavor = LsFlavor::of(BSD_PLATFORM, Path::new(cmd.get_program()));
    let (argv, parsed) = SplitArgv::new(&LsGrammar(flavor), args);
    let show_all = parsed.show_all;
    // In a long listing, permission info is preserved as octal.
    let show_long = parsed.show_long;

    cmd.env("LC_ALL", "C");
    cmd.arg(if parsed.wants_all { "-la" } else { "-l" });
    let (literal, operands) = ls_argv(args, &argv, &parsed);
    cmd.args(literal.iter().map(|arg| &**arg));
    if let Some(operands) = operands {
        // With no operand ls lists `.`.
        cmd.glob_args(&operands);
    }

    let label = if args.is_empty() {
        ".".to_string()
    } else {
        args.join(" ")
    };

    runner::run_filtered(
        cmd,
        "ls",
        &label,
        |raw| {
            let (entries, parsed_count, truncated, filtered) = compact_ls(raw, show_all, show_long);

            // If no lines were parsed (e.g., unrecognized locale), fall back to raw output.
            // This is safer than returning "(empty)" for a non-empty directory.
            let has_real_content = raw
                .lines()
                .any(|l| !l.starts_with("total ") && !l.is_empty() && !is_dotdir(l));
            if parsed_count == 0 && has_real_content {
                return raw.to_string();
            }

            let mut out = entries;

            if let Some(hint) = hidden_hint(&truncated, &filtered) {
                out.push_str(&hint);
                out.push('\n');
            }

            if verbose > 0 {
                eprintln!(
                    "Chars: {} → {} ({}% reduction)",
                    raw.len(),
                    out.len(),
                    if !raw.is_empty() {
                        100usize.saturating_sub(out.len() * 100 / raw.len())
                    } else {
                        0
                    }
                );
            }
            out
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

/// Build the recovery hint for entries dropped from the listing —
/// truncated past the display cap and/or RTK-filtered noise.
///
/// Standard RTK pattern: a truncation note plus a one-shot command to
/// retrieve the remaining. The tee file contains ONLY the dropped
/// entries (truncated first, then filtered), so `tail -n +1` (whole
/// file) retrieves nothing the agent has already seen.
fn hidden_hint(truncated: &[String], filtered: &[String]) -> Option<String> {
    if truncated.is_empty() && filtered.is_empty() {
        return None;
    }
    let note = match (truncated.len(), filtered.len()) {
        (0, f) => format!("... ({} filtered)", f),
        (t, 0) => format!("... ({} more)", t),
        (t, f) => format!("... ({} more, {} filtered)", t, f),
    };
    let mut hidden_only = String::new();
    for line in truncated.iter().chain(filtered) {
        hidden_only.push_str(line);
        hidden_only.push('\n');
    }
    match crate::core::tee::force_tee_tail_hint(&hidden_only, "ls-hidden", 1) {
        Some(tee_hint) => Some(format!("{}\n{}", note, tee_hint)),
        None => Some(note),
    }
}

/// Format bytes into human-readable size
fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

/// Parse a single `ls -la` line, returning `(file_type_char, perms, size, name)`.
///
/// `perms` is the raw 10-char string from ls (e.g. `-rw-r--r--`); use
/// [`perms_to_octal`] to render it.
///
/// Uses the date field as a stable anchor — the date format in `ls -la` is
/// always three tokens (`Mon DD HH:MM` or `Mon DD  YYYY`), so we locate it
/// with a regex, then extract size (rightmost number before the date) and
/// filename (everything after the date). This handles owner/group names that
/// contain spaces, which break the old fixed-column approach.
fn parse_ls_line(line: &str) -> Option<(char, String, u64, String)> {
    // Skip . and .. entries before date parsing (works for non-English locales too)
    if is_dotdir(line) {
        return None;
    }

    let date_match = LS_DATE_RE.find(line)?;
    let name = line[date_match.end()..].to_string();

    let before_date = &line[..date_match.start()];
    let before_parts: Vec<&str> = before_date.split_whitespace().collect();
    if before_parts.len() < 4 {
        return None;
    }

    let perms = before_parts[0].to_string();
    let file_type = perms.chars().next()?;

    // Size is the rightmost parseable number before the date.
    // nlinks is also numeric but appears earlier; scanning from the end
    // guarantees we hit the size field first.
    let mut size: u64 = 0;
    for part in before_parts.iter().rev() {
        if let Ok(s) = part.parse::<u64>() {
            size = s;
            break;
        }
    }

    Some((file_type, perms, size, name))
}

/// Returns true if the line represents a . or .. directory entry.
///
/// POSIX.1-2017 (IEEE Std 1003.1) specifies that each directory contains
/// entries for "." (the directory itself) and ".." (its parent). These entries
/// always appear in `ls -la` output and are skipped during parsing since they
/// carry no meaningful content for token reduction.
fn is_dotdir(line: &str) -> bool {
    line.trim().ends_with('.') || line.trim().ends_with("..")
}

/// Convert an `ls`-style permission string (e.g. `-rw-r--r--`, `drwxr-xr-x`,
/// `-rwsr-xr-t`) into octal notation (e.g. `644`, `755`, `4755`).
///
/// Returns `None` if the input does not look like a permission field.
/// Special bits (setuid/setgid/sticky) are encoded as a leading 4th digit when
/// any are set; otherwise we emit a 3-digit value to stay compact.
fn perms_to_octal(perms: &str) -> Option<String> {
    if perms.len() < 10 || !perms.is_ascii() {
        return None;
    }
    let b = perms.as_bytes();

    fn perm_value(read: bool, write: bool, exec: bool) -> u32 {
        ((read as u32) << 2) | ((write as u32) << 1) | (exec as u32)
    }

    let owner_x = matches!(b[3], b'x' | b's');
    let group_x = matches!(b[6], b'x' | b's');
    let other_x = matches!(b[9], b'x' | b't');

    let owner = perm_value(b[1] == b'r', b[2] == b'w', owner_x);
    let group = perm_value(b[4] == b'r', b[5] == b'w', group_x);
    let other = perm_value(b[7] == b'r', b[8] == b'w', other_x);

    let setuid = matches!(b[3], b's' | b'S');
    let setgid = matches!(b[6], b's' | b'S');
    let sticky = matches!(b[9], b't' | b'T');
    let special = perm_value(setuid, setgid, sticky);

    if special > 0 {
        Some(format!("{}{}{}{}", special, owner, group, other))
    } else {
        Some(format!("{}{}{}", owner, group, other))
    }
}

/// Parse ls -la output into compact format.
///
/// Without `show_long`:
///   name/        (dirs)
///   name  size   (files)
///
/// With `show_long` (user passed `-l`):
///   755  name/        (dirs)
///   644  name  size   (files)
///
/// Returns (entries, parsed_count, truncated, filtered) so caller can emit
/// a recovery hint when anything was dropped.
/// parsed_count tracks how many non-header lines were successfully parsed.
/// truncated holds compact lines beyond the CAP_INVENTORY display cap.
/// filtered holds the display name of each entry RTK removed from view
/// (noise dirs without -a/-A as `name/`, plus raw unparsable non-dotdir
/// lines).
/// If parsed_count == 0 but raw had content, caller should fall back to raw output.
fn compact_ls(
    raw: &str,
    show_all: bool,
    show_long: bool,
) -> (String, usize, Vec<String>, Vec<String>) {
    let mut dirs: Vec<(String, Option<String>)> = Vec::new(); // (name, octal_perms)
    let mut files: Vec<(String, String, Option<String>)> = Vec::new(); // (name, size, octal_perms)
    let mut lines_seen: usize = 0;
    let mut parsed_count: usize = 0;
    let mut dotdirs: usize = 0;
    let mut filtered: Vec<String> = Vec::new();

    for line in raw.lines() {
        if line.starts_with("total ") || line.is_empty() {
            continue;
        }
        lines_seen += 1;

        let Some((file_type, perms, size, name)) = parse_ls_line(line) else {
            if is_dotdir(line) {
                dotdirs += 1;
            } else {
                filtered.push(line.trim().to_string());
            }
            continue;
        };
        parsed_count += 1;

        // Filter noise dirs unless dotfiles were requested; every entry the
        // child printed and RTK drops is recorded so the hint can recover it.
        if !show_all && NOISE_DIRS.iter().any(|noise| name == *noise) {
            filtered.push(format!("{}/", name));
            continue;
        }

        // Only parse perms when the user actually wants the long listing —
        // skip the work otherwise.
        let octal = if show_long {
            perms_to_octal(&perms)
        } else {
            None
        };

        if file_type == 'd' {
            dirs.push((name, octal));
        } else {
            // Regular files, symlinks, character/block devices, pipes, sockets
            files.push((name, human_size(size), octal));
        }
    }

    if dirs.is_empty() && files.is_empty() {
        if lines_seen > 0 && parsed_count == 0 {
            if dotdirs == lines_seen {
                // Only . and .. entries (empty directory)
                return ("(empty)\n".to_string(), 0, Vec::new(), Vec::new());
            }
            // Real content that couldn't be parsed (e.g., non-English locale)
            return (String::new(), 0, Vec::new(), Vec::new());
        }
        // Everything parsed was filtered out (e.g., only noise dirs) —
        // keep filtered so the caller can still emit a recovery hint.
        return ("(empty)\n".to_string(), parsed_count, Vec::new(), filtered);
    }

    // Dirs first, then files — one compact line each
    let mut all_lines: Vec<String> = Vec::with_capacity(dirs.len() + files.len());
    for (name, octal) in &dirs {
        all_lines.push(match octal {
            Some(octal) => format!("{}  {}/", octal, name),
            None => format!("{}/", name),
        });
    }
    for (name, size, octal) in &files {
        all_lines.push(match octal {
            Some(octal) => format!("{}  {}  {}", octal, name, size),
            None => format!("{}  {}", name, size),
        });
    }

    // Cap the displayed listing; the rest is recoverable via the tee hint.
    let truncated = if all_lines.len() > CAP_INVENTORY {
        all_lines.split_off(CAP_INVENTORY)
    } else {
        Vec::new()
    };

    let mut entries = String::new();
    for line in &all_lines {
        entries.push_str(line);
        entries.push('\n');
    }

    (entries, parsed_count, truncated, filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_basic() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 .\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 ..\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 README.md\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(entries.contains("src/"));
        assert!(entries.contains("Cargo.toml"));
        assert!(entries.contains("README.md"));
        assert!(entries.contains("1.2K")); // 1234 bytes
        assert!(entries.contains("5.5K")); // 5678 bytes
        assert!(!entries.contains("drwx")); // no permissions
        assert!(!entries.contains("staff")); // no group
        assert!(!entries.contains("total")); // no total
        assert!(!entries.contains("\n.\n")); // no . entry
        assert!(!entries.contains("\n..\n")); // no .. entry
    }

    #[test]
    fn test_compact_filters_noise() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(!entries.contains("node_modules"));
        assert!(!entries.contains(".git"));
        assert!(!entries.contains("target"));
        assert!(entries.contains("src/"));
        assert!(entries.contains("main.rs"));
    }

    #[test]
    fn test_compact_hidden_noise_dirs() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 target\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert_eq!(
            hidden,
            vec!["node_modules/", ".git/", "target/"],
            "every noise dir the child printed is recorded"
        );

        let (_entries, _parsed, _truncated, hidden_all) = compact_ls(input, true, false);
        assert!(hidden_all.is_empty(), "-a shows noise dirs, nothing hidden");
    }

    #[test]
    fn test_compact_hidden_unparsable_line() {
        // A non-dotdir line the date regex can't parse is silently dropped —
        // it must be collected as hidden so the caller emits a recovery hint.
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n\
                     garbage line without date anchor\n";
        let (entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert!(entries.contains("main.rs"));
        assert_eq!(hidden, vec!["garbage line without date anchor"]);
    }

    #[test]
    fn test_compact_hidden_clean_listing() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert!(hidden.is_empty(), "nothing dropped, no hint expected");
    }

    #[test]
    fn test_compact_only_noise_dirs_keeps_hidden() {
        // Directory containing only noise dirs: output collapses to (empty),
        // but RTK-filtered entries must survive so the agent knows they exist.
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n";
        let (entries, parsed, _truncated, hidden) = compact_ls(input, false, false);
        assert_eq!(entries, "(empty)\n");
        assert_eq!(parsed, 2);
        assert_eq!(hidden, vec!["node_modules/", ".git/"]);
    }

    #[test]
    fn test_shows_dotfiles_flags() {
        let s = |v: &[&str]| LsGrammar(LsFlavor::Gnu).split(v).1.show_all;
        assert!(s(&["-a"]));
        assert!(s(&["-A"]));
        assert!(s(&["-lA", "."]));
        assert!(s(&["--all"]));
        assert!(s(&["--almost-all"]));
        assert!(!s(&["-l", "."]));
        assert!(!s(&["--author"]));
        // A flag's value is not a flag: `-I a*` ignores `a*`, it does not ask for dotfiles.
        assert!(!s(&["-I", "a*"]));
        assert!(!s(&["-Ia*"]));
    }

    #[test]
    fn test_compact_records_dot_noise_dir_when_child_printed_it() {
        // Child ran with -A but RTK was told not to show all: the dot noise
        // dir must be recorded, never silently vanish.
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 node_modules\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 README.md\n";
        let (entries, _parsed, _truncated, filtered) = compact_ls(input, false, false);
        assert!(!entries.contains(".git"));
        assert_eq!(filtered, vec![".git/", "node_modules/"]);
    }

    #[test]
    fn test_hidden_hint_none_when_empty() {
        assert!(hidden_hint(&[], &[]).is_none());
    }

    #[test]
    fn test_hidden_hint_truncation_note_and_one_shot_command() {
        let noise = vec!["node_modules/".to_string(), "target/".to_string()];
        let hint = hidden_hint(&[], &noise).expect("hint for hidden entries");
        assert!(hint.starts_with("... (2 filtered)"));
        assert!(
            !hint.contains("use -a"),
            "standard ls flags are not RTK's job to teach: {hint}"
        );
        assert!(
            !hint.contains("full output"),
            "must not point at already-seen output: {hint}"
        );
        // Recovery availability depends on environment; when present the hint
        // is the standard one-shot retrieval command over the hidden entries.
        if hint.lines().count() > 1 {
            assert!(
                hint.contains("[see remaining: tail -n +1 ")
                    || hint.contains("hidden: rtk recall "),
                "recovery hint must be a standard retrieval form: {hint}"
            );
        }
    }

    #[test]
    fn test_hidden_hint_note_variants() {
        let t = vec!["x  1B".to_string()];
        let f = vec!["target/".to_string()];
        assert!(
            hidden_hint(&t, &[])
                .expect("hint")
                .starts_with("... (1 more)")
        );
        assert!(
            hidden_hint(&[], &f)
                .expect("hint")
                .starts_with("... (1 filtered)")
        );
        assert!(
            hidden_hint(&t, &f)
                .expect("hint")
                .starts_with("... (1 more, 1 filtered)")
        );
    }

    #[test]
    fn test_compact_truncates_past_cap() {
        let mut input = String::from("total 0\n");
        for i in 0..60 {
            input.push_str(&format!(
                "-rw-r--r--  1 user  staff  100 Jan  1 12:00 file{:02}.txt\n",
                i
            ));
        }
        let (entries, _parsed, truncated, _hidden) = compact_ls(&input, false, false);
        assert_eq!(entries.lines().count(), CAP_INVENTORY);
        assert_eq!(truncated.len(), 60 - CAP_INVENTORY);
        assert!(entries.contains("file00.txt"));
        assert!(!entries.contains("file59.txt"));
        assert!(
            truncated.iter().any(|l| l.contains("file59.txt")),
            "overflow entries must be recoverable via the tee file"
        );
    }

    #[test]
    fn test_compact_no_truncation_under_cap() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  100 Jan  1 12:00 main.rs\n";
        let (_entries, _parsed, truncated, _hidden) = compact_ls(input, false, false);
        assert!(truncated.is_empty());
    }

    #[test]
    fn test_compact_show_all() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 .git\n\
                     drwxr-xr-x  2 user  staff  64 Jan  1 12:00 src\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, true, false);
        assert!(entries.contains(".git/"));
        assert!(entries.contains("src/"));
    }

    #[test]
    fn test_compact_empty() {
        let input = "total 0\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(entries, "(empty)\n");
    }

    #[test]
    fn test_compact_empty_chinese_locale() {
        let input = "total 8\n\
                     drwxr-xr-x  2 user user  4096  1月  1 12:00 .\n\
                     drwxr-xr-x 16 user user 20480  1月  1 12:00 ..\n";
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert_eq!(entries, "(empty)\n");
    }

    #[test]
    fn test_compact_empty_english_locale() {
        let input = "total 0\n\
                     drwxr-xr-x  2 lumin  wheel  64 Apr 23 00:37 .\n\
                     drwxr-xr-x 16 root  wheel 164576 Apr 23 00:37 ..\n";
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert_eq!(entries, "(empty)\n");
    }

    #[test]
    fn test_human_size() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(500), "500B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(1234), "1.2K");
        assert_eq!(human_size(1_048_576), "1.0M");
        assert_eq!(human_size(2_500_000), "2.4M");
    }

    #[test]
    fn test_compact_handles_filenames_with_spaces() {
        let input = "total 8\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 my file.txt\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(entries.contains("my file.txt"));
    }

    #[test]
    fn test_compact_symlinks() {
        let input = "total 8\n\
                     lrwxr-xr-x  1 user  staff  10 Jan  1 12:00 link -> target\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(entries.contains("link -> target"));
    }

    #[test]
    fn test_entries_no_summary() {
        // No summary line anywhere — pure entries (agent-first output)
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            !entries.contains("Summary:"),
            "entries must not contain summary"
        );
    }

    #[test]
    fn test_pipe_line_count() {
        // Simulates: rtk ls | wc -l
        // Entries should have exactly 1 line per file/dir, no extra blank or summary
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 main.rs\n\
                     -rw-r--r--  1 user  staff  5678 Jan  1 12:00 lib.rs\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        let line_count = entries.lines().count();
        assert_eq!(
            line_count, 3,
            "pipe should see exactly 3 lines (1 dir + 2 files), got {}",
            line_count
        );
    }

    // Regression test for #948: owner/group with spaces breaks fixed-column parsing
    #[test]
    fn test_compact_multiline_group() {
        let input = "total 8\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine    0 Mar 31 16:18 empty.txt\n\
                     -rw-r--r--  1 fjeanne utilisa. du domaine 1234 Mar 31 16:18 data.json\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("empty.txt"),
            "should contain 'empty.txt', got: {entries}"
        );
        assert!(
            entries.contains("data.json"),
            "should contain 'data.json', got: {entries}"
        );
        assert!(
            !entries.contains("16:18"),
            "time should not leak into filename, got: {entries}"
        );
        assert!(
            entries.contains("0B"),
            "empty.txt should show 0B, got: {entries}"
        );
        assert!(
            entries.contains("1.2K"),
            "data.json should show 1.2K (1234 bytes), got: {entries}"
        );
    }

    #[test]
    fn test_compact_year_format_date() {
        // Some systems show year instead of time for old files
        let input = "total 8\n\
                     -rw-r--r--  1 user staff  5678 Dec 25  2024 archive.tar\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("archive.tar"),
            "should contain filename, got: {entries}"
        );
        assert!(entries.contains("5.5K"), "should show 5.5K, got: {entries}");
    }

    #[test]
    fn test_parse_ls_line_basic() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 user staff 1234 Jan  1 12:00 file.txt").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 1234);
        assert_eq!(name, "file.txt");
    }

    #[test]
    fn test_parse_ls_line_multiline_group() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 fjeanne utilisa. du domaine 0 Mar 31 16:18 empty.txt")
                .unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 0);
        assert_eq!(name, "empty.txt");
    }

    #[test]
    fn test_parse_ls_line_dir_with_space_in_group() {
        let (ft, perms, size, name) =
            parse_ls_line("drwxr-xr-x  2 fjeanne utilisa. du domaine 64 Mar 31 16:18 my dir")
                .unwrap();
        assert_eq!(ft, 'd');
        assert_eq!(perms, "drwxr-xr-x");
        assert_eq!(size, 64);
        assert_eq!(name, "my dir");
    }

    #[test]
    fn test_parse_ls_line_symlink() {
        let (ft, perms, size, name) =
            parse_ls_line("lrwxr-xr-x  1 user staff 10 Jan  1 12:00 link -> target").unwrap();
        assert_eq!(ft, 'l');
        assert_eq!(perms, "lrwxr-xr-x");
        assert_eq!(size, 10);
        assert_eq!(name, "link -> target");
    }

    #[test]
    fn test_compact_device_files() {
        // Regression test for #844: `rtk ls /dev/ttyACM*` returned "(empty)"
        // because character devices (type 'c') were not handled by compact_ls.
        let input = "crw-rw----  1 root  dialout  166, 0 Apr 22 09:46 /dev/ttyACM0\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/ttyACM0"),
            "should contain device file, got: {entries}"
        );
        assert!(!entries.contains("(empty)"), "should not be empty");
    }

    #[test]
    fn test_compact_device_files_macos_hex_size() {
        // macOS shows device major/minor as hex (e.g. 0x2000000)
        let input = "crw-rw-rw-  1 root  wheel  0x2000000 Mar 31 19:25 /dev/tty\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/tty"),
            "should contain device file, got: {entries}"
        );
    }

    #[test]
    fn test_compact_block_device() {
        let input = "brw-rw----  1 root  disk  8, 0 Apr 22 09:46 /dev/sda\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            entries.contains("/dev/sda"),
            "should contain block device, got: {entries}"
        );
    }

    #[test]
    fn test_parse_ls_line_returns_none_for_total() {
        assert!(parse_ls_line("total 48").is_none());
    }

    #[test]
    fn test_parse_ls_line_year_format() {
        let (ft, perms, size, name) =
            parse_ls_line("-rw-r--r--  1 user staff 5678 Dec 25  2024 old.tar.gz").unwrap();
        assert_eq!(ft, '-');
        assert_eq!(perms, "-rw-r--r--");
        assert_eq!(size, 5678);
        assert_eq!(name, "old.tar.gz");
    }

    #[test]
    fn test_perms_to_octal_common() {
        assert_eq!(perms_to_octal("-rw-r--r--").as_deref(), Some("644"));
        assert_eq!(perms_to_octal("-rwxr-xr-x").as_deref(), Some("755"));
        assert_eq!(perms_to_octal("drwxr-xr-x").as_deref(), Some("755"));
        assert_eq!(perms_to_octal("-rw-------").as_deref(), Some("600"));
        assert_eq!(perms_to_octal("-rwxrwxrwx").as_deref(), Some("777"));
        assert_eq!(perms_to_octal("----------").as_deref(), Some("000"));
        assert_eq!(perms_to_octal("lrwxr-xr-x").as_deref(), Some("755"));
    }

    #[test]
    fn test_perms_to_octal_special_bits() {
        // setuid + 755 -> 4755
        assert_eq!(perms_to_octal("-rwsr-xr-x").as_deref(), Some("4755"));
        // setuid without execute -> 4644
        assert_eq!(perms_to_octal("-rwSr--r--").as_deref(), Some("4644"));
        // setgid + 755 -> 2755
        assert_eq!(perms_to_octal("-rwxr-sr-x").as_deref(), Some("2755"));
        // sticky bit on /tmp-style dir -> 1777
        assert_eq!(perms_to_octal("drwxrwxrwt").as_deref(), Some("1777"));
        // setuid + setgid + sticky
        assert_eq!(perms_to_octal("-rwsrwsrwt").as_deref(), Some("7777"));
    }

    #[test]
    fn test_perms_to_octal_garbage() {
        assert_eq!(perms_to_octal(""), None);
        assert_eq!(perms_to_octal("short"), None);
    }

    #[test]
    fn test_compact_long_format_includes_octal() {
        let input = "total 48\n\
                     drwxr-xr-x  2 user  staff    64 Jan  1 12:00 src\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n\
                     -rwxr-xr-x  1 user  staff   500 Jan  1 12:00 build.sh\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, true);
        assert!(
            entries.contains("755  src/"),
            "dir should be prefixed with octal perms, got: {entries}"
        );
        assert!(
            entries.contains("644  Cargo.toml  1.2K"),
            "file should be prefixed with octal perms, got: {entries}"
        );
        assert!(
            entries.contains("755  build.sh  500B"),
            "executable should show 755, got: {entries}"
        );
    }

    #[test]
    fn test_compact_short_format_omits_octal() {
        // Without -l, no octal prefix even though we still parse `ls -la`
        // under the hood.
        let input = "total 48\n\
                     -rw-r--r--  1 user  staff  1234 Jan  1 12:00 Cargo.toml\n";
        let (entries, _parsed, _truncated, _hidden) = compact_ls(input, false, false);
        assert!(
            !entries.contains("644"),
            "short format must not include octal perms, got: {entries}"
        );
        assert!(entries.contains("Cargo.toml"));
    }

    #[test]
    fn test_compact_chinese_locale_fallback() {
        let input = "total 8\n\
                      drwxr-xr-x  2 user staff  64  1月  1 12:00 src\n\
                      -rw-r--r--  1 user staff 1234  1月  1 12:00 main.rs\n";
        let (entries, parsed_count, _truncated, _hidden) = compact_ls(input, false, false);
        assert_eq!(parsed_count, 0);
        assert!(entries.is_empty());
    }

    use LsFlavor::{Bsd, Gnu};

    fn operands<'a>(flavor: LsFlavor, args: &[&'a str]) -> (Vec<&'a str>, bool) {
        let (split, _) = LsGrammar(flavor).split(args);
        (
            split.indices.iter().map(|&i| args[i]).collect(),
            split.bounded,
        )
    }

    /// What `run` appends after `-l`/`-la`, operands as text.
    fn sent(flavor: LsFlavor, args: &[&str]) -> Vec<String> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let (argv, parsed) = SplitArgv::new(&LsGrammar(flavor), &args);
        let (literal, operands) = ls_argv(&args, &argv, &parsed);
        let mut sent: Vec<String> = literal.into_iter().map(Cow::into_owned).collect();
        sent.extend(operands.iter().flat_map(|ops| ops.iter().cloned()));
        sent
    }

    fn parsed(flavor: LsFlavor, args: &[&str]) -> LsArgs {
        LsGrammar(flavor).split(args).1
    }

    #[test]
    fn the_flavor_is_the_resolved_ls() {
        assert!(is_system_ls(Path::new("/bin/ls")));
        for gnu in [
            "/opt/homebrew/opt/coreutils/libexec/gnubin/ls",
            "/usr/local/opt/coreutils/libexec/gnubin/ls",
            "/opt/homebrew/bin/ls",
            "/usr/bin/ls",
            "ls",
        ] {
            assert!(!is_system_ls(Path::new(gnu)), "{gnu}");
        }
        // Off a BSD platform the path is not even looked at.
        assert_eq!(LsFlavor::of(false, Path::new("/bin/ls")), Gnu);
        assert_eq!(
            LsFlavor::of(false, Path::new(r"C:\Program Files\Git\usr\bin\ls.exe")),
            Gnu
        );
        // A gnubin path that does not canonicalize here is compared as it is.
        assert_eq!(
            LsFlavor::of(
                true,
                Path::new("/opt/homebrew/opt/coreutils/libexec/gnubin/ls")
            ),
            Gnu
        );
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        assert_eq!(LsFlavor::of(true, Path::new("/bin/../bin/ls")), Bsd);
        // A resolved path with `..` names the same ls as its canonical form (where there is
        // a `/bin/ls` to resolve).
        #[cfg(unix)]
        assert_eq!(
            canonical_program(Path::new("/bin/../bin/ls")),
            canonical_program(Path::new("/bin/ls"))
        );
        // Nothing to canonicalize: the path is kept as it is.
        let missing = Path::new("/definitely/missing/../ls");
        assert_eq!(canonical_program(missing), missing);
    }

    #[test]
    fn a_separate_value_follows_the_flavor_that_runs() {
        // GNU's `-T`, `-w` and `-I` take the next argument; BSD's are booleans, and BSD ls stops
        // reading flags at its first operand, so the operand goes last.
        assert_eq!(sent(Gnu, &["-T", "src", "-t"]), ["-T", "src", "-t"]);
        assert_eq!(sent(Bsd, &["-T", "src", "-t"]), ["-T", "-t", "src"]);
        assert_eq!(sent(Gnu, &["-lT", "src", "-r"]), ["-T", "src", "-r"]);
        assert_eq!(sent(Bsd, &["-lT", "src", "-r"]), ["-T", "-r", "src"]);
        assert_eq!(sent(Gnu, &["-I", "--all", "x"]), ["-I", "--all", "x"]);
        assert!(!parsed(Gnu, &["-I", "--all", "x"]).wants_all);
        assert_eq!(sent(Bsd, &["-I", "--all", "x"]), ["-I", "x"]);
        assert!(parsed(Bsd, &["-I", "--all", "x"]).wants_all);
        // GNU's `-D` is `--dired`; BSD's takes a format.
        assert_eq!(sent(Gnu, &["-lD", "src*"]), ["-D", "src*"]);
        assert_eq!(operands(Gnu, &["-lD", "src*"]), (vec!["src*"], true));
        assert_eq!(sent(Bsd, &["-lD", "src*"]), ["-D", "src*"]);
        assert_eq!(operands(Bsd, &["-lD", "src*"]), (vec![], true));
        assert_eq!(operands(Bsd, &["-D", "%F", "src*"]), (vec!["src*"], true));
        assert_eq!(
            operands(Gnu, &["-D", "%F", "src*"]),
            (vec!["%F", "src*"], true)
        );
        assert_eq!(operands(Bsd, &["-I", "src*"]), (vec!["src*"], true));
        assert_eq!(operands(Gnu, &["-I", "src*"]), (vec![], true));
    }

    #[test]
    fn a_required_value_takes_a_literal_double_dash() {
        // GNU ls reads `-I -- -lh src` as the pattern `--`, then `-lh`, then `src`.
        assert_eq!(sent(Gnu, &["-I", "--", "-lh", "src"]), ["-I", "--", "src"]);
        assert!(parsed(Gnu, &["-I", "--", "-lh", "src"]).show_long);
        assert_eq!(sent(Gnu, &["-I", "--", "-a"]), ["-I", "--"]);
        assert!(parsed(Gnu, &["-I", "--", "-a"]).wants_all);
        assert_eq!(operands(Gnu, &["--hide", "--", "src"]), (vec!["src"], true));
        assert_eq!(operands(Bsd, &["-D", "--", "src"]), (vec!["src"], true));
    }

    #[test]
    fn a_flag_value_stays_with_its_flag_and_out_of_the_operands() {
        // #4325: `-R` was taken as the pattern and `*.md` as a path.
        assert_eq!(
            sent(Gnu, &["-I", "*.md", "-R", "src/core"]),
            ["-I", "*.md", "-R", "src/core"]
        );
        assert_eq!(
            operands(Gnu, &["-I", "*.md", "-R", "src/core"]),
            (vec!["src/core"], true)
        );
        assert_eq!(
            operands(Gnu, &["-w", "80", "-T", "4", "."]),
            (vec!["."], true)
        );
        assert_eq!(
            operands(Gnu, &["--hide", "*.md", "src*"]),
            (vec!["src*"], true)
        );
        assert_eq!(
            operands(Gnu, &["--ignore=*.md", "src*"]),
            (vec!["src*"], true)
        );
    }

    #[test]
    fn inside_a_cluster_the_bytes_after_a_value_letter_are_kept() {
        for flavor in [Gnu, Bsd] {
            // #4325: `-Ihello` reached ls as `-Ieo`, its `l` and `h` dropped as flags.
            assert_eq!(sent(flavor, &["-Ihello"]), ["-Ihello"]);
            assert_eq!(sent(flavor, &["-laIhello", "src"]), ["-Ihello", "src"]);
            assert_eq!(sent(flavor, &["-Ta"]), ["-Ta"]);
            assert_eq!(sent(flavor, &["-lD%Y"]), ["-D%Y"]);
            assert_eq!(sent(flavor, &["-lw80"]), ["-w80"]);
            assert_eq!(sent(flavor, &["--all", "-lah"]), Vec::<String>::new());
        }
    }

    #[test]
    fn bsd_ls_reads_i_t_and_w_as_booleans() {
        // BSD `-Ta` is `-T -a`; GNU `-Ta` is a tab size of `a`.
        assert!(parsed(Bsd, &["-Ta"]).show_all);
        assert!(!parsed(Gnu, &["-Ta"]).show_all);
        assert!(parsed(Bsd, &["-wa"]).show_all);
        assert!(parsed(Bsd, &["-Ia"]).show_all);
        // `--all` is rtk's own on BSD too, where ls has no long options to speak of.
        assert_eq!(sent(Bsd, &["--all", "src"]), ["src"]);
        assert!(parsed(Bsd, &["--all"]).wants_all);
        for flavor in [Gnu, Bsd] {
            assert!(parsed(flavor, &["-laIhello"]).wants_all);
            assert!(parsed(flavor, &["-lD%Y"]).show_long);
            assert!(!parsed(flavor, &["-w80"]).show_long);
        }
    }

    #[test]
    fn a_bounded_argv_sends_flags_before_operands() {
        for flavor in [Gnu, Bsd] {
            assert_eq!(sent(flavor, &["src", "-t"]), ["-t", "src"]);
            assert_eq!(sent(flavor, &["a", "--", "-b"]), ["--", "a", "-b"]);
        }
        assert_eq!(sent(Gnu, &["src", "-I", "x"]), ["-I", "x", "src"]);
    }

    #[test]
    fn a_long_flag_is_read_under_the_name_it_abbreviates() {
        // `--sor size` is GNU ls's `--sort size`.
        assert_eq!(
            operands(Gnu, &["src/core", "--sor", "size"]),
            (vec!["src/core"], true)
        );
        assert_eq!(
            sent(Gnu, &["src/core", "--sor", "size"]),
            ["--sor", "size", "src/core"]
        );
        assert!(parsed(Gnu, &["--almost"]).show_all);
        assert!(parsed(Gnu, &["--form", "long"]).show_long);
    }

    #[test]
    fn an_unknown_or_ambiguous_long_flag_keeps_the_typed_order() {
        // `--hid` is both `--hide` and `--hide-control-chars`.
        assert!(!operands(Gnu, &["src", "--hid", "x"]).1);
        assert_eq!(sent(Gnu, &["src", "--hid", "x"]), ["src", "--hid", "x"]);
        // BSD ls has no `--hide`; an attached value keeps the split bounded.
        assert!(!operands(Bsd, &["--hide", "x"]).1);
        assert!(operands(Bsd, &["--hide=x", "src"]).1);
    }

    #[test]
    fn an_optional_value_is_read_only_when_attached() {
        assert_eq!(operands(Gnu, &["--color", "never"]), (vec!["never"], true));
        assert_eq!(
            operands(Gnu, &["--color=never", "src"]),
            (vec!["src"], true)
        );
    }

    #[test]
    fn a_long_listing_is_detected_from_flags_only() {
        let long = |v: &[&str]| shows_long(&ls_tokens(Gnu, v), Gnu);
        assert!(long(&["-l"]));
        assert!(long(&["--format", "long"]));
        assert!(long(&["--format=verbose"]));
        assert!(!long(&["-I", "long"]));
        assert!(!long(&["-Ilong"]));
    }
}

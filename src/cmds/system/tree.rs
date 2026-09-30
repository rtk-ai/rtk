//! tree command - proxy to native tree with token-optimized output
//!
//! This module proxies to the native `tree` command and filters the output
//! to reduce token usage while preserving structure visibility.
//!
//! Token optimization: automatically excludes noise directories via -I pattern
//! unless -a flag is present (respecting user intent).

use super::constants::NOISE_DIRS;
use crate::core::child_command::{OperandGrammar, OperandSplit, SplitArgv};
use crate::core::runner::{self, RunOptions};
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;

/// tree's boolean long options (`tree --help`, v2.3.2), which it matches exactly.
const TREE_LONG_FLAGS: &[&str] = &[
    "acl",
    "condense",
    "device",
    "dirsfirst",
    "du",
    "fflinks",
    "filesfirst",
    "fromfile",
    "fromtabfile",
    "gitignore",
    "help",
    "hyperlink",
    "ignore-case",
    "info",
    "inodes",
    "matchdirs",
    "metafirst",
    "nolinks",
    "noreport",
    "opt-toggle",
    "prune",
    "selinux",
    "si",
    "version",
];

/// tree's value-taking long options. tree matches these by prefix: an argument that starts
/// with one takes its value after an `=` right behind the name, and otherwise from the next
/// argument (`--sortx=name src` sorts by `src`).
const TREE_LONG_VALUES: &[&str] = &[
    "authority",
    "charset",
    "compress",
    "filelimit",
    "gitfile",
    "hintro",
    "houtro",
    "infofile",
    "scheme",
    "sort",
    "timefmt",
];

/// tree's value-taking letters. Each takes the next argument, wherever it sits in its cluster
/// and whatever that argument looks like (`-Pd '*.md'` and `-dP '*.md'` alike, `-P -d` takes
/// `-d` as the pattern). `-L` alone also reads a level written right behind it (`-L1`).
const TREE_VALUE_LETTERS: &[char] = &['H', 'I', 'L', 'P', 'T', 'o'];

/// What tree's own option loop makes of an argv.
///
/// tree's parser is not getopt's: a value letter in the middle of a cluster takes the next
/// argument, and value-taking long options match by prefix, neither of which the
/// arg_tokenizer can express, so this walks the argv the way tree does.
pub(crate) struct TreeArgs {
    operands: Vec<usize>,
    bounded: bool,
    show_all: bool,
    has_ignore: bool,
}

fn parse_tree_args<T: AsRef<str>>(args: &[T]) -> TreeArgs {
    let mut parsed = TreeArgs {
        operands: Vec::new(),
        bounded: true,
        show_all: false,
        has_ignore: false,
    };
    let mut options = true;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_ref();
        // The next argument not yet taken as a value.
        let mut next = i + 1;
        if !options || !arg.starts_with('-') || arg.len() == 1 {
            parsed.operands.push(i);
        } else if arg == "--" {
            options = false;
        } else if let Some(long) = arg.strip_prefix("--") {
            if let Some(name) = TREE_LONG_VALUES
                .iter()
                .find(|name| long.starts_with(**name))
            {
                if !long[name.len()..].starts_with('=') {
                    next += 1;
                }
            } else if matches!(long, "fromfile" | "fromtabfile") {
                // The operands then name files holding a listing, not directories.
                parsed.bounded = false;
            } else if !TREE_LONG_FLAGS.contains(&long) {
                // tree rejects it; nothing about the operands can be trusted.
                parsed.bounded = false;
            }
        } else {
            let mut letters = arg[1..].chars().peekable();
            while let Some(letter) = letters.next() {
                match letter {
                    'a' => parsed.show_all = true,
                    'L' if letters.peek().is_some_and(char::is_ascii_digit) => {
                        while letters.next_if(char::is_ascii_digit).is_some() {}
                    }
                    letter if TREE_VALUE_LETTERS.contains(&letter) => {
                        parsed.has_ignore |= letter == 'I';
                        next += 1;
                    }
                    _ => {}
                }
            }
        }
        i = next;
    }
    parsed
}

/// tree's grammar, as [`parse_tree_args`] reads it.
pub(crate) struct TreeGrammar;

impl OperandGrammar for TreeGrammar {
    type Parsed = TreeArgs;

    fn split<T: AsRef<str>>(&self, args: &[T]) -> (OperandSplit, TreeArgs) {
        let mut parsed = parse_tree_args(args);
        let split = OperandSplit {
            indices: std::mem::take(&mut parsed.operands),
            bounded: parsed.bounded,
        };
        (split, parsed)
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if !tool_exists("tree") {
        anyhow::bail!(
            "tree command not found. Install it first:\n\
             - macOS: brew install tree\n\
             - Ubuntu/Debian: sudo apt install tree\n\
             - Fedora/RHEL: sudo dnf install tree\n\
             - Arch: sudo pacman -S tree"
        );
    }

    let mut cmd = resolved_command("tree");

    let (argv, parsed) = SplitArgv::new(&TreeGrammar, args);
    if !parsed.show_all && !parsed.has_ignore {
        let ignore_pattern = NOISE_DIRS.join("|");
        cmd.arg("-I").arg(&ignore_pattern);
    }

    cmd.split_args(&argv);

    runner::run_filtered(
        cmd,
        "tree",
        &args.join(" "),
        |raw| {
            let filtered = filter_tree_output(raw);
            if verbose > 0 {
                eprintln!(
                    "Lines: {} → {} ({}% reduction)",
                    raw.lines().count(),
                    filtered.lines().count(),
                    if raw.lines().count() > 0 {
                        100 - (filtered.lines().count() * 100 / raw.lines().count())
                    } else {
                        0
                    }
                );
            }
            filtered
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

fn filter_tree_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    if lines.is_empty() {
        return "\n".to_string();
    }

    let mut filtered_lines = Vec::new();

    for line in lines {
        // Skip the final summary line (e.g., "5 directories, 23 files")
        if line.contains("director") && line.contains("file") {
            continue;
        }

        // Skip empty lines at the end
        if line.trim().is_empty() && filtered_lines.is_empty() {
            continue;
        }

        filtered_lines.push(line);
    }

    // Remove trailing empty lines
    while filtered_lines.last().is_some_and(|l| l.trim().is_empty()) {
        filtered_lines.pop();
    }

    filtered_lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_removes_summary() {
        let input = ".\n├── src\n│   └── main.rs\n└── Cargo.toml\n\n2 directories, 3 files\n";
        let output = filter_tree_output(input);
        assert!(!output.contains("directories"));
        assert!(!output.contains("files"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("Cargo.toml"));
    }

    #[test]
    fn test_filter_preserves_structure() {
        let input = ".\n├── src\n│   ├── main.rs\n│   └── lib.rs\n└── tests\n    └── test.rs\n";
        let output = filter_tree_output(input);
        assert!(output.contains("├──"));
        assert!(output.contains("│"));
        assert!(output.contains("└──"));
        assert!(output.contains("main.rs"));
        assert!(output.contains("test.rs"));
    }

    #[test]
    fn test_filter_handles_empty() {
        let input = "";
        let output = filter_tree_output(input);
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_filter_removes_trailing_empty_lines() {
        let input = ".\n├── file.txt\n\n\n";
        let output = filter_tree_output(input);
        assert_eq!(output.matches('\n').count(), 2); // Root + file.txt + final newline
    }

    #[test]
    fn test_filter_summary_variations() {
        // Test different summary formats
        let inputs = vec![
            (".\n└── file.txt\n\n0 directories, 1 file\n", "1 file"),
            (".\n└── file.txt\n\n1 directory, 0 files\n", "1 directory"),
            (".\n└── file.txt\n\n10 directories, 25 files\n", "25 files"),
        ];

        for (input, summary_fragment) in inputs {
            let output = filter_tree_output(input);
            assert!(
                !output.contains(summary_fragment),
                "Should remove summary '{}' from output",
                summary_fragment
            );
            assert!(
                output.contains("file.txt"),
                "Should preserve file.txt in output"
            );
        }
    }

    #[test]
    fn test_noise_dirs_constant() {
        // Verify NOISE_DIRS contains expected patterns
        assert!(NOISE_DIRS.contains(&"node_modules"));
        assert!(NOISE_DIRS.contains(&".git"));
        assert!(NOISE_DIRS.contains(&"target"));
        assert!(NOISE_DIRS.contains(&"__pycache__"));
        assert!(NOISE_DIRS.contains(&".next"));
        assert!(NOISE_DIRS.contains(&"dist"));
        assert!(NOISE_DIRS.contains(&"build"));
    }

    fn operands<'a>(args: &[&'a str]) -> (Vec<&'a str>, bool) {
        let (split, _) = TreeGrammar.split(args);
        (
            split.indices.iter().map(|&i| args[i]).collect(),
            split.bounded,
        )
    }

    // Each row was checked against tree v2.3.2 itself.

    #[test]
    fn a_value_letter_takes_the_next_argument_wherever_it_sits() {
        assert_eq!(operands(&["-P", "*.rs", "src*"]), (vec!["src*"], true));
        assert_eq!(operands(&["-Pd", "*.md", "src"]), (vec!["src"], true));
        assert_eq!(operands(&["-dP", "*.md", "src"]), (vec!["src"], true));
        assert_eq!(operands(&["-PL", "*.md", "1", "."]), (vec!["."], true));
        // Blindly: `-P -d` searches for `-d`.
        assert_eq!(operands(&["-P", "-d", "src"]), (vec!["src"], true));
        assert_eq!(
            operands(&["-I", "target", "-L", "2", "."]),
            (vec!["."], true)
        );
    }

    #[test]
    fn a_value_letter_takes_a_literal_double_dash() {
        // tree v2.3.2 reads `-P -- src` as the pattern `--` and lists `src`.
        assert_eq!(operands(&["-P", "--", "src"]), (vec!["src"], true));
        assert_eq!(operands(&["--charset", "--", "src"]), (vec!["src"], true));
        // `-I --` ignores `--`, and `-a` is a flag again.
        let parsed = parse_tree_args(&["-I", "--", "-a"]);
        assert!(parsed.has_ignore && parsed.show_all);
    }

    #[test]
    fn a_level_may_be_attached_to_l() {
        assert_eq!(operands(&["-L1", "."]), (vec!["."], true));
        assert_eq!(operands(&["-L1d", "."]), (vec!["."], true));
        assert_eq!(operands(&["-Ld", "1", "."]), (vec!["."], true));
    }

    #[test]
    fn a_long_value_option_matches_by_prefix() {
        assert_eq!(
            operands(&["--charset", "ascii", "src"]),
            (vec!["src"], true)
        );
        assert_eq!(operands(&["--sort=name", "src"]), (vec!["src"], true));
        assert_eq!(operands(&["--sortx", "name", "src"]), (vec!["src"], true));
        // Not `=` right behind the name, so the value is the next argument.
        assert_eq!(operands(&["--sortx=name", "src"]), (vec![], true));
        assert_eq!(operands(&["--noreport", "src"]), (vec!["src"], true));
        assert_eq!(operands(&["--", "-d"]), (vec!["-d"], true));
    }

    #[test]
    fn a_listing_file_or_an_unknown_flag_leaves_the_operands_unbounded() {
        assert!(!operands(&["--fromfile", "list*.txt"]).1);
        assert!(!operands(&["--noreportx", "src"]).1);
    }

    #[test]
    fn show_all_and_ignore_are_read_from_clusters() {
        // `rtk tree -ad` must not hide the noise directories.
        let parsed = parse_tree_args(&["-ad"]);
        assert!(parsed.show_all && !parsed.has_ignore);
        let parsed = parse_tree_args(&["-dI", "x"]);
        assert!(parsed.has_ignore && !parsed.show_all);
        // A pattern is not a flag: `-P -a` searches for `-a`.
        assert!(!parse_tree_args(&["-P", "-a"]).show_all);
    }
}

//! Every delegate that hands a command to `rtk rewrite` must end the option
//! list with `--` first. Without it, a command that reads as an option of
//! `rtk rewrite` itself (`--help`, `-h`) is parsed by clap, which prints usage
//! text and exits 0, the "rewrite, allowed" answer, so the delegate runs the
//! usage text as the command (#5127).
mod common;

use std::path::Path;

/// Each delegate and the exact call it must make.
const DELEGATES: &[(&str, &str)] = &[
    (
        "hooks/pi/rtk.ts",
        r#"pi.exec("rtk", ["rewrite", "--", cmd]"#,
    ),
    (
        "hooks/hermes/rtk-rewrite/__init__.py",
        r#"["rtk", "rewrite", "--", command]"#,
    ),
    (
        "openclaw/index.ts",
        r#"execFileSync("rtk", ["rewrite", "--", command]"#,
    ),
    ("hooks/claude/rtk-rewrite.sh", r#"rtk rewrite -- "$CMD""#),
    ("hooks/cursor/rtk-rewrite.sh", r#"rtk rewrite -- "$CMD""#),
    (".claude/hooks/rtk-rewrite.sh", r#"rtk rewrite -- "$CMD""#),
];

#[test]
fn every_rewrite_delegate_ends_options_before_the_command() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let missing: Vec<&str> = DELEGATES
        .iter()
        .filter(|(file, call)| {
            let source = std::fs::read_to_string(root.join(file))
                .unwrap_or_else(|err| panic!("read {file}: {err}"));
            !source.contains(call)
        })
        .map(|(file, _)| *file)
        .collect();
    assert!(
        missing.is_empty(),
        "these delegates call `rtk rewrite` without `--` before the command: {missing:?}"
    );
}

/// The contract the delegates rely on: after `--`, an option-shaped command
/// is a command to look up, not a flag of `rtk rewrite`, so it gets no
/// rewrite and no usage text.
#[test]
fn rewrite_after_double_dash_treats_an_option_as_the_command() {
    for command in ["--help", "-h"] {
        let output = common::rtk_command()
            .args(["rewrite", "--", command])
            .output()
            .expect("run rtk rewrite");
        assert_ne!(output.status.code(), Some(0), "{command}: {output:?}");
        assert!(output.stdout.is_empty(), "{command}: {output:?}");
    }
}

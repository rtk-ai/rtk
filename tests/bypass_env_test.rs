//! `rtk bypass` hands leading `NAME=value` words to the command's environment instead of
//! looking the first of them up on PATH as the program.

mod common;

/// A command that prints its environment as `NAME=value` lines. On Unix it is `sh -c env`
/// rather than `env`, which would itself read an assignment left in its argv as one. `/D`
/// keeps cmd.exe from running the registry's AutoRun command first.
#[cfg(windows)]
const PRINT_ENVIRONMENT: [&str; 4] = ["cmd", "/D", "/C", "set"];
#[cfg(not(windows))]
const PRINT_ENVIRONMENT: [&str; 3] = ["sh", "-c", "env"];

/// `MSG` arrives literally only if no shell reads the line: `$HOME*` stays unexpanded and
/// `;`, `|` and `&` stay characters.
const ASSIGNMENTS: [&str; 3] = [
    "KUBEBUILDER_ASSETS=/x/bin",
    "GOFLAGS=-mod=mod",
    "MSG=$HOME*;a|b&c",
];

#[test]
fn leading_assignments_reach_the_command_environment() {
    let words: Vec<String> = ASSIGNMENTS
        .iter()
        .chain(PRINT_ENVIRONMENT.iter())
        .map(|s| s.to_string())
        .collect();
    // As separate arguments, and as one quoted argument that rtk splits.
    for argv in [words.clone(), vec![words.join(" ")]] {
        let out = common::rtk_command()
            .arg("bypass")
            .args(&argv)
            .output()
            .expect("spawn rtk bypass");
        assert!(
            out.status.success(),
            "{argv:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        for assignment in ASSIGNMENTS {
            assert!(
                stdout.lines().any(|line| line == assignment),
                "{argv:?}: {assignment} missing from\n{stdout}"
            );
        }
    }
}

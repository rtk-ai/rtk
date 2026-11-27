#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn lint_preserves_eslint_write_and_stdin_scope() {
    let dir = common::tempdir();
    let args_file = dir.path().join("arguments");
    let tool = dir.path().join("eslint");
    std::fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$LINT_ARGS_FILE\"\nprintf '[]\\n'\n",
    )
    .expect("write ESLint fixture");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("make ESLint fixture executable");

    for (args, expected) in [
        (vec!["--fix"], "-f\njson\n--fix\n"),
        (vec!["--fix-dry-run"], "-f\njson\n--fix-dry-run\n"),
        (
            vec!["--pass-on-no-patterns"],
            "-f\njson\n--pass-on-no-patterns\n",
        ),
        (
            vec!["--config", "config with spaces.js", "--fix"],
            "-f\njson\n--config\nconfig with spaces.js\n--fix\n",
        ),
        (
            vec!["--stdin", "--fix", "--stdin-filename=x.js"],
            "-f\njson\n--stdin\n--fix\n--stdin-filename=x.js\n",
        ),
        (
            vec!["--stdin", "--stdin-filename", "src/file with spaces.js"],
            "-f\njson\n--stdin\n--stdin-filename\nsrc/file with spaces.js\n",
        ),
        (vec!["--fix", "src/"], "-f\njson\n--fix\nsrc/\n"),
        (vec![], "-f\njson\n.\n"),
        (vec!["--cache"], "-f\njson\n--cache\n.\n"),
        (
            vec!["--ignore-pattern", "--fix"],
            "-f\njson\n--ignore-pattern\n--fix\n.\n",
        ),
        (vec!["file=name.js"], "-f\njson\nfile=name.js\n"),
        (vec!["--", "--fix"], "-f\njson\n--\n--fix\n"),
    ] {
        let output = common::rtk_command()
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("LINT_ARGS_FILE", &args_file)
            .args(["lint", "eslint"])
            .args(&args)
            .output()
            .expect("run RTK lint");
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert_eq!(
            std::fs::read_to_string(&args_file).expect("read child arguments"),
            expected,
            "{args:?}"
        );
    }
}

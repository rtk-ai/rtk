use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn npx_eslint_preserves_linter_and_bare_path_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    #[cfg(unix)]
    let (filename, script) = (
        "eslint",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGS_FILE\"\nprintf '[]\\n'\nexit 1\n",
    );
    #[cfg(windows)]
    let (filename, script) = (
        "eslint.cmd",
        "@echo off\r\n(for %%a in (%*) do @echo %%~a) > \"%ARGS_FILE%\"\r\necho []\r\nexit /b 1\r\n",
    );
    let eslint = dir.path().join(filename);
    fs::write(&eslint, script).expect("write eslint fixture");
    #[cfg(unix)]
    fs::set_permissions(&eslint, fs::Permissions::from_mode(0o755)).expect("chmod eslint");

    for path in ["src", "lib", "src/", "."] {
        let args_file = dir.path().join("args");
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .current_dir(dir.path())
            .env("PATH", dir.path())
            .env("ARGS_FILE", &args_file)
            .env("RTK_DB_PATH", dir.path().join("tracking.db"))
            .args(["npx", "eslint", path])
            .output()
            .expect("run rtk");

        assert_eq!(
            fs::read_to_string(&args_file)
                .expect("eslint should execute")
                .lines()
                .collect::<Vec<_>>(),
            vec!["-f", "json", path],
            "eslint should receive the user path unchanged"
        );
        assert_eq!(output.status.code(), Some(1), "preserve linter exit code");
        fs::remove_file(args_file).expect("remove captured args");
    }
}

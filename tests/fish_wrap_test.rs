//! End-to-end coverage for the fish-script wrap: `rtk rewrite` emits the
//! `rtk run --shell fish -c '<script>'` form, the quoting round-trips under
//! POSIX and fish host layers, and `rtk run` executes the wrapped script.

#[cfg(unix)]
mod unix {
    use std::process::{Command, Output};

    const FISH_BLOCK: &str = "if test -d src\n  git status\nelse\n  echo missing\nend";

    fn rtk() -> Command {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
    }

    /// Run `rtk rewrite` with an isolated home so user config and Claude Code
    /// settings never leak into the verdict or the wrap gate.
    fn rewrite_isolated(command: &str, config_toml: Option<&str>) -> Output {
        let home = tempfile::tempdir().expect("create isolated home");
        if let Some(content) = config_toml {
            // dirs::config_dir() resolves differently per platform; cover both.
            for dir in [
                home.path().join("Library/Application Support/rtk"),
                home.path().join("rtk"),
            ] {
                std::fs::create_dir_all(&dir).expect("create config dir");
                std::fs::write(dir.join("config.toml"), content).expect("write config");
            }
        }
        rtk()
            .args(["rewrite", command])
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .expect("run rtk rewrite")
    }

    /// Both branches assert: with `fish` the hook must wrap, without it the
    /// wrap must be skipped rather than half-applied. A test that returns early
    /// asserts nothing on a runner with no `fish`, which is most of them.
    #[test]
    fn rewrite_wraps_multiline_fish_block_as_ask() {
        let output = rewrite_isolated(FISH_BLOCK, None);

        if which::which("fish").is_err() {
            assert_eq!(
                output.status.code(),
                Some(1),
                "without fish the wrap must defer"
            );
            assert!(output.stdout.is_empty());
            return;
        }

        assert_eq!(output.status.code(), Some(3), "wrap must surface as Ask");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("rtk run --shell fish -c '{FISH_BLOCK}'")
        );
        assert!(output.stderr.is_empty());
    }

    /// The wrap carries the rewrite the command would have had without it.
    #[test]
    fn rewrite_keeps_the_inner_rewrite_inside_the_wrap() {
        let output = rewrite_isolated("git diff HEAD~3 HEAD; and true", None);
        let rewritten = String::from_utf8_lossy(&output.stdout);

        if which::which("fish").is_err() {
            // The leading command is rewritable on its own, so without a `fish`
            // to wrap for this is an ordinary rewrite — not a defer. Asserting
            // the exact string is what pins the wrap as genuinely off.
            assert_eq!(output.status.code(), Some(3));
            assert_eq!(rewritten, "rtk git diff HEAD~3 HEAD; and true");
            return;
        }

        assert_eq!(output.status.code(), Some(3));
        assert_eq!(
            rewritten,
            "rtk run --shell fish -c 'rtk git diff HEAD~3 HEAD; and true'"
        );
    }

    /// A script the permission gate could not decompose is never emitted as an
    /// `rtk`-prefixed command, with or without a local `fish`.
    #[test]
    fn rewrite_defers_unattestable_fish_scripts() {
        for command in [
            "test -d src; and cat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and git status # don't\ncat secrets.env > /tmp/rtk-leak-test",
            "test -d src; and echo $(whoami)",
            "for f in (ls)\n  echo $f\nend",
        ] {
            let output = rewrite_isolated(command, None);

            assert_eq!(output.status.code(), Some(1), "{command:?}");
            assert!(output.stdout.is_empty(), "{command:?}");
        }
    }

    #[test]
    fn rewrite_honors_wrap_fish_scripts_opt_out() {
        let output = rewrite_isolated(FISH_BLOCK, Some("[hooks]\nwrap_fish_scripts = false\n"));

        assert_eq!(output.status.code(), Some(1), "opt-out must defer");
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn rewrite_defers_posix_multiline_script() {
        let output = rewrite_isolated("if [ -d src ]; then\n  git status\nfi", None);

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn rewrite_defers_fish_script_with_divergent_backslash() {
        // Classifies fish (`; and` marker) but contains `\\`, which fish
        // single-quotes would collapse — the wrapper vetoes it, so the hook
        // defers instead of emitting a wrap that would corrupt the script.
        let output = rewrite_isolated("echo '\\\\'; and echo ok", None);

        assert_eq!(
            output.status.code(),
            Some(1),
            "divergent backslash must defer"
        );
        assert!(output.stdout.is_empty());
    }

    /// Mirror of `fish_script::escape_single_quoted` — tests/ cannot reach
    /// crate internals, and duplicating one replace keeps this test honest
    /// about what the emitted command actually contains.
    fn escape_single_quoted(script: &str) -> String {
        script.replace('\'', "'\\''")
    }

    #[test]
    fn wrapped_quoting_round_trips_under_posix_and_fish_hosts() {
        // The `%s\n` carries a lone backslash inside single quotes: literal
        // under both POSIX and fish, so it must arrive byte-identical (unlike
        // `\\`/`\'`, which the wrapper vetoes upstream).
        let script = "if test -d src\n  printf '%s\\n' 'a b'\nelse\n  echo missing\nend";
        let probe = format!("printf '%s' '{}'", escape_single_quoted(script));

        for shell in ["sh", "zsh", "fish"] {
            let Ok(shell_path) = which::which(shell) else {
                continue;
            };
            let output = Command::new(shell_path)
                .args(["-c", &probe])
                .output()
                .unwrap_or_else(|e| panic!("run {shell}: {e}"));
            assert!(output.status.success(), "{shell} must parse the wrapping");
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                script,
                "script must arrive byte-identical through a {shell} host layer"
            );
        }
    }

    #[test]
    #[ignore] // executes real fish; run with: cargo test --ignored
    fn wrapped_multiline_fish_script_executes_and_propagates_exit_code() {
        let Ok(fish) = which::which("fish") else {
            return;
        };
        let script = "if test -d /\n  echo yes\nelse\n  echo no\nend";

        let direct = Command::new(&fish)
            .args(["-c", script])
            .output()
            .expect("run fish directly");
        let wrapped = rtk()
            .args(["run", "--shell", "fish", "-c", script])
            .output()
            .expect("run rtk run");

        assert!(wrapped.status.success());
        assert_eq!(
            wrapped.stdout, direct.stdout,
            "output must match direct fish"
        );

        let status = rtk()
            .args(["run", "--shell", "fish", "-c", "exit 2"])
            .status()
            .expect("run rtk run");
        assert_eq!(status.code(), Some(2), "child exit code must propagate");
    }
}

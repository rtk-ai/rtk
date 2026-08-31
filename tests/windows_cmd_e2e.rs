#![cfg(windows)]

use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn native_cmd(expression: &str) -> Output {
    Command::new("cmd.exe")
        .args(["/D", "/S", "/C", expression])
        .output()
        .expect("native cmd.exe should start")
}

fn rtk_cmd(expression: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", expression])
        .output()
        .expect("rtk cmd should start")
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn assert_cmd_parity(expression: &str) {
    let native = native_cmd(expression);
    let rtk = rtk_cmd(expression);

    assert_eq!(rtk.status.code(), native.status.code(), "{expression}");
    assert_eq!(rtk.stdout, native.stdout, "{expression}");
    assert_eq!(rtk.stderr, native.stderr, "{expression}");
}

/// A native `dir` footer includes free disk space, which can change between
/// the two intentionally separate process launches while unrelated tests tee
/// artifacts. Compare every stable native byte and mask only that live value.
fn assert_dir_parity(expression: &str) {
    let native = native_cmd(expression);
    let rtk = rtk_cmd(expression);

    assert_eq!(rtk.status.code(), native.status.code(), "{expression}");
    assert_eq!(rtk.stderr, native.stderr, "{expression}");
    assert_eq!(
        dir_stdout_without_live_free_space(&rtk.stdout),
        dir_stdout_without_live_free_space(&native.stdout),
        "{expression}"
    );
}

fn dir_stdout_without_live_free_space(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| {
            line.split_once(" Dir(s)").map_or_else(
                || line.to_owned(),
                |(prefix, _)| format!("{prefix} Dir(s) <free>"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_cmd_parity_in(expression: &str, current_dir: &Path) {
    let native = Command::new("cmd.exe")
        .current_dir(current_dir)
        .args(["/D", "/S", "/C", expression])
        .output()
        .expect("native cmd.exe should start");
    let rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .current_dir(current_dir)
        .args(["cmd", expression])
        .output()
        .expect("rtk cmd should start");

    assert!(native.status.success(), "native cmd failed: {expression}");
    assert_eq!(rtk.status.code(), native.status.code(), "{expression}");
    assert_eq!(rtk.stdout, native.stdout, "{expression}");
    assert_eq!(rtk.stderr, native.stderr, "{expression}");
}

#[test]
fn query_chains_keep_cmd_operator_and_stateful_semantics() {
    assert_cmd_parity("echo %CD% & dir /b");
    assert_cmd_parity("set RTK_CMD_E2E=kept & echo %RTK_CMD_E2E% & set RTK_CMD_E2E=");
    assert_cmd_parity("cd /D . & dir /b");
    assert_cmd_parity("cmd /D /S /C \"exit /b 0\" && echo success || echo failure");
    assert_cmd_parity("cmd /D /S /C \"exit /b 1\" && echo success || echo failure");
}

#[test]
fn unicode_spaces_and_failures_have_native_parity() {
    let directory = tempdir().unwrap();
    let unicode_dir = directory.path().join("spaced Привет");
    fs::create_dir(&unicode_dir).unwrap();
    fs::write(unicode_dir.join("данные.txt"), "payload").unwrap();

    assert_cmd_parity_in("dir /b", &unicode_dir);
    assert_cmd_parity("exit /b 37");
}

#[test]
fn redirection_and_batch_input_fail_open_to_native_cmd() {
    let directory = tempdir().unwrap();
    let redirected = directory.path().join("listing.txt");
    let batch = directory.path().join("returns-23.cmd");
    fs::write(&batch, "@echo batch:%~1\r\n@exit /b 23\r\n").unwrap();

    let redirect_expression = format!("echo redirected > {}", redirected.display());
    assert_cmd_parity(&redirect_expression);
    assert_eq!(
        fs::read_to_string(&redirected).unwrap().trim(),
        "redirected"
    );

    assert_cmd_parity(&format!("{} hello", batch.display()));

    let input = directory.path().join("input.txt");
    fs::write(&input, "input through redirect\r\n").unwrap();
    assert_cmd_parity(&format!("type < {}", input.display()));
}

#[test]
fn multi_argument_embedded_quote_and_metacharacters_do_not_execute_an_extra_command() {
    let directory = tempdir().unwrap();
    let injected = directory.path().join("must-not-exist.txt");
    let payload = format!(r#"safe" & echo injected > {}"#, injected.display());

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", &payload])
        .output()
        .expect("rtk cmd should start");

    assert!(output.status.success());
    assert_eq!(output.stdout, format!("{payload}\r\n").as_bytes());
    assert!(
        !injected.exists(),
        "embedded metacharacters must stay data, not execute a redirected command"
    );

    let nested_payload = "nested \"quoted\" value";
    let nested = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "cmd.exe", "/D", "/C", "echo", nested_payload])
        .output()
        .expect("nested CMD should start");
    let native_nested = Command::new("cmd.exe")
        .args(["/D", "/C", "echo", nested_payload])
        .output()
        .expect("native nested CMD should start");
    assert_eq!(nested.status.code(), native_nested.status.code());
    assert_eq!(nested.stdout, native_nested.stdout);
    assert_eq!(nested.stderr, native_nested.stderr);

    let nested_injected = directory.path().join("nested-must-not-exist.txt");
    let nested_payload = format!("'safe & echo injected > {}'", nested_injected.display());
    let nested = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "cmd.exe", "/D", "/C", "echo", &nested_payload])
        .output()
        .expect("nested CMD should start");
    assert!(!nested.status.success());
    assert!(String::from_utf8_lossy(&nested.stderr).contains("one raw expression"));
    assert!(
        !nested_injected.exists(),
        "nested CMD metacharacters must remain data"
    );

    for (index, operator) in ["&", "|", ">"].into_iter().enumerate() {
        let marker = directory
            .path()
            .join(format!("nested-operator-{index}.txt"));
        let nested = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args([
                "cmd",
                "cmd.exe",
                "/D",
                "/C",
                "echo",
                operator,
                "echo",
                "injected",
                ">",
                marker.to_str().unwrap(),
            ])
            .output()
            .expect("nested CMD should start");
        assert!(!nested.status.success());
        assert!(String::from_utf8_lossy(&nested.stderr).contains("one raw expression"));
        assert!(!marker.exists(), "nested {operator:?} must not execute");
    }

    let absolute_cmd =
        std::env::var_os("ComSpec").unwrap_or_else(|| "C:\\Windows\\System32\\cmd.exe".into());
    let absolute_cmd = absolute_cmd.to_string_lossy();
    for (index, suffix) in ["", ".", " "].into_iter().enumerate() {
        let spelled_cmd = format!("{absolute_cmd}{suffix}");
        let absolute_marker = directory
            .path()
            .join(format!("absolute-cmd-operator-{index}.txt"));
        let absolute = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args([
                "cmd",
                &spelled_cmd,
                "/D",
                "/C",
                "echo",
                "&",
                "echo",
                "injected",
                ">",
                absolute_marker.to_str().unwrap(),
            ])
            .output()
            .expect("absolute nested CMD should start");
        assert!(!absolute.status.success());
        assert!(String::from_utf8_lossy(&absolute.stderr).contains("one raw expression"));
        assert!(
            !absolute_marker.exists(),
            "absolute cmd.exe must not bypass syntax checks"
        );
    }

    let nested_directory = directory.path().join("folder with spaces");
    fs::create_dir(&nested_directory).unwrap();
    let nested_file = nested_directory.join("nested.txt");
    fs::write(&nested_file, "nested file payload\r\n").unwrap();
    let nested = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "cmd",
            "cmd.exe",
            "/D",
            "/C",
            "type",
            nested_file.to_str().unwrap(),
        ])
        .output()
        .expect("nested CMD should start");
    let native_nested = Command::new("cmd.exe")
        .args(["/D", "/C", "type", nested_file.to_str().unwrap()])
        .output()
        .expect("native nested CMD should start");
    assert_eq!(nested.status.code(), native_nested.status.code());
    assert_eq!(nested.stdout, native_nested.stdout);
    assert_eq!(nested.stderr, native_nested.stderr);
}

#[test]
fn multi_argument_empty_and_bang_values_match_default_cmd_semantics() {
    let empty_rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", ""])
        .output()
        .expect("rtk cmd should start");
    assert!(empty_rtk.status.success());
    assert_eq!(empty_rtk.stdout, b"\"\"\r\n");
    assert!(empty_rtk.stderr.is_empty());

    let bang_native = native_cmd("echo !RTK_CMD_UNSET!");
    let bang_rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", "!RTK_CMD_UNSET!"])
        .output()
        .expect("rtk cmd should start");
    assert_eq!(bang_rtk.status.code(), bang_native.status.code());
    assert_eq!(bang_rtk.stdout, bang_native.stdout);
    assert_eq!(bang_rtk.stderr, bang_native.stderr);
}

#[test]
fn multi_argument_python_argv_matches_native_for_spaces_quotes_and_metacharacters() {
    let directory = tempdir().unwrap();
    let injected = directory.path().join("argv-injected.txt");
    let code = "import json, sys; print(json.dumps(sys.argv[1:]))";
    let rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "cmd",
            "python",
            "-c",
            code,
            "hello world",
            r#"{"a": 1}"#,
            "",
            "!RTK_CMD_UNSET!",
            "%TEMP% value",
            "a b\\",
            "a\\\"b",
            &format!("safe & echo injected > {}", injected.display()),
        ])
        .output()
        .expect("rtk cmd should start");

    assert!(rtk.status.success());
    let expected = serde_json::json!([
        "hello world",
        r#"{"a": 1}"#,
        "",
        "!RTK_CMD_UNSET!",
        "%TEMP% value",
        "a b\\",
        "a\\\"b",
        format!("safe & echo injected > {}", injected.display()),
    ]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&rtk.stdout).unwrap(),
        expected
    );
    assert!(
        !injected.exists(),
        "argv metacharacters must stay data, not execute a redirected command"
    );
}

#[test]
fn multi_argument_commands_do_not_expose_transport_environment_variables() {
    let prefix_native = native_cmd("set RTK_CMD_ARG_");
    let prefix_rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "set", "RTK_CMD_ARG_"])
        .output()
        .expect("rtk cmd should start");
    assert_eq!(prefix_rtk.status.code(), prefix_native.status.code());
    assert_eq!(prefix_rtk.stdout, prefix_native.stdout);
    assert_eq!(prefix_rtk.stderr, prefix_native.stderr);

    let bare_native = native_cmd("cmd.exe /D /C set");
    let bare_rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "cmd.exe", "/D", "/C", "set"])
        .output()
        .expect("rtk cmd should start");
    assert_eq!(bare_rtk.status.code(), bare_native.status.code());
    assert_eq!(bare_rtk.stdout, bare_native.stdout);
    assert_eq!(bare_rtk.stderr, bare_native.stderr);
    assert!(!String::from_utf8_lossy(&bare_rtk.stdout).contains("RTK_CMD_ARG_"));

    let hidden_rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "cmd.exe", "/D", "/C", "set\r\n"])
        .output()
        .expect("rtk cmd nested syntax check should start");
    assert!(!hidden_rtk.status.success());
    assert!(String::from_utf8_lossy(&hidden_rtk.stderr).contains("one raw expression"));
}

#[test]
fn multi_argument_percent_and_crlf_payloads_remain_data() {
    let directory = tempdir().unwrap();
    let percent_injected = directory.path().join("percent-must-not-exist.txt");
    let percent_payload = format!(
        "100% complete & echo injected > {}",
        percent_injected.display()
    );
    let percent_output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", &percent_payload])
        .output()
        .expect("rtk cmd should start");
    assert!(percent_output.status.success());
    assert_eq!(
        percent_output.stdout,
        format!("{percent_payload}\r\n").as_bytes()
    );
    assert!(
        !percent_injected.exists(),
        "percent-bearing payload must not create a redirected marker"
    );

    let crlf_injected = directory.path().join("crlf-must-not-exist.txt");
    let crlf_payload = format!(
        "first line\r\n& echo injected > {}",
        crlf_injected.display()
    );
    let crlf_output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", &crlf_payload])
        .output()
        .expect("rtk cmd should start");
    assert!(crlf_output.status.success());
    assert_eq!(crlf_output.stdout, format!("{crlf_payload}\r\n").as_bytes());
    assert!(
        !crlf_injected.exists(),
        "CR/LF payload must not create a redirected marker"
    );

    let quoted_crlf_payload = "first \"quoted\" line\r\nsecond line";
    let quoted_crlf_output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "echo", quoted_crlf_payload])
        .output()
        .expect("rtk cmd should start");
    assert!(!quoted_crlf_output.status.success());
    assert!(String::from_utf8_lossy(&quoted_crlf_output.stderr).contains("cannot safely carry"));
}

#[test]
fn hidden_transport_bounds_the_cmd_line_when_a_percent_operand_is_present() {
    let long_operand = "x".repeat(8_050);
    let native = native_cmd(&format!("dir %RTK_CMD_LITERAL% {long_operand}"));
    let rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "dir", "%RTK_CMD_LITERAL%", &long_operand])
        .output()
        .expect("rtk cmd should start");

    assert_eq!(rtk.status.code(), native.status.code());
    assert!(!String::from_utf8_lossy(&rtk.stderr).contains("command line is too long"));
}

#[test]
fn multi_argument_python_line_break_payload_executes_once_and_matches_native() {
    let line_break = "first line\r\nsecond line with spaces & echo injected > marker.txt";
    let trailing_slash = "tail\r\nslash\\";
    let code = "import json, sys; print(json.dumps(sys.argv[1:]))";
    let rtk = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "cmd",
            "python",
            "-c",
            code,
            line_break,
            "second arg",
            trailing_slash,
        ])
        .output()
        .expect("rtk cmd should start");

    assert!(rtk.status.success());
    let expected = serde_json::json!([line_break, "second arg", trailing_slash]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&rtk.stdout).unwrap(),
        expected
    );
    assert!(
        !String::from_utf8_lossy(&rtk.stdout).contains("RTK_INTERNAL_CMD_"),
        "line-break transport must clear every hidden key before the target starts"
    );
}

#[test]
fn multi_argument_pipe_input_redirection_and_caret_payloads_remain_data() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("input.txt");
    fs::write(&input, "input data\r\n").unwrap();

    for (label, payload) in [
        (
            "pipe",
            format!(
                "safe | echo injected > {}",
                directory.path().join("pipe-marker.txt").display()
            ),
        ),
        (
            "input-redirection",
            format!(
                "safe < {} > {}",
                input.display(),
                directory.path().join("input-marker.txt").display()
            ),
        ),
        (
            "caret",
            format!(
                "safe ^ & echo injected > {}",
                directory.path().join("caret-marker.txt").display()
            ),
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["cmd", "echo", &payload])
            .output()
            .expect("rtk cmd should start");

        assert!(output.status.success(), "{label}");
        assert_eq!(
            output.stdout,
            format!("{payload}\r\n").as_bytes(),
            "{label}"
        );
    }

    for marker in ["pipe-marker.txt", "input-marker.txt", "caret-marker.txt"] {
        assert!(
            !directory.path().join(marker).exists(),
            "{marker} must not be created by transported data"
        );
    }
}

#[test]
fn machine_consumed_builtin_output_is_native_even_for_structured_display_commands() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("visible.txt"), "payload").unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested").join("deep.txt"), "payload").unwrap();
    let spaced = directory.path().join("folder with spaces");
    fs::create_dir(&spaced).unwrap();
    fs::write(spaced.join("spaced.txt"), "payload").unwrap();

    assert_dir_parity(&format!("dir /a:-d /o:n {}", directory.path().display()));

    let mut set_display = (0..32)
        .map(|index| format!("set RTK_CMD_E2E_FILTER_{index:02}=alpha"))
        .collect::<Vec<_>>();
    set_display.push("set RTK_CMD_E2E_FILTER".to_owned());
    assert_cmd_parity(&set_display.join(" & "));

    assert_cmd_parity_in("dir /b", directory.path());
    assert_dir_parity(&format!(
        "dir /a:-d /o:n /t:w {}",
        directory.path().display()
    ));
    assert_dir_parity(&format!(
        "dir /s /a:-d {}\\*.txt",
        directory.path().display()
    ));
    assert_dir_parity(&format!("dir /a:-d /o:n \"{}\"", spaced.display()));
    let mut native_multiarg_command = Command::new("cmd.exe");
    native_multiarg_command
        .args(["/D", "/S", "/C"])
        .raw_arg(format!("dir \"{}\"", spaced.display()));
    let native_multiarg_dir = native_multiarg_command
        .output()
        .expect("native cmd.exe should start");
    let captured_multiarg_dir = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmd", "dir", spaced.to_str().unwrap()])
        .output()
        .expect("rtk cmd should start");
    assert_eq!(
        captured_multiarg_dir.status.code(),
        native_multiarg_dir.status.code()
    );
    assert_eq!(
        dir_stdout_without_live_free_space(&captured_multiarg_dir.stdout),
        dir_stdout_without_live_free_space(&native_multiarg_dir.stdout),
        "captured multi-argument DIR output must stay native"
    );
    assert_eq!(captured_multiarg_dir.stderr, native_multiarg_dir.stderr);
    assert_cmd_parity("help assoc");
    assert_cmd_parity("set RTK_CMD_MISSING_FILTER_PREFIX");
    assert_cmd_parity("assoc .rtk_missing_extension");
    assert_cmd_parity("ftype RTK_MISSING_FILETYPE");
}

#[test]
fn combined_dir_b_switches_keep_exact_native_output_when_captured() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("visible.txt"), "payload").unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested").join("deep.txt"), "payload").unwrap();

    for switches in ["/s/b", "/b/s", "/a-d/b", "/A-D/B"] {
        assert_cmd_parity(&format!("dir {switches} {}", directory.path().display()));
    }
}

#[test]
fn hidden_cmd_runner_keeps_caret_escaped_dir_b_native() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("visible.txt"), "payload").unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::write(directory.path().join("nested").join("deep.txt"), "payload").unwrap();

    for switches in ["/^b", "/s/^b", "/^B/s", "/s^/b", "/S^/B"] {
        let source = format!("dir {switches} {}", directory.path().display());
        let native = native_cmd(&source);
        let hidden = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["__cmd-run", "--hex", &hex_encode(source.as_bytes())])
            .output()
            .expect("hidden cmd runner should start");

        assert_eq!(hidden.status.code(), native.status.code(), "{source}");
        assert_eq!(
            dir_stdout_without_live_free_space(&hidden.stdout),
            dir_stdout_without_live_free_space(&native.stdout),
            "{source}"
        );
        assert_eq!(hidden.stderr, native.stderr, "{source}");
    }
}

#[test]
fn alternate_and_interactive_dir_layouts_stay_native_in_the_hidden_runner() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("visible.txt"), "payload").unwrap();
    fs::create_dir(directory.path().join("nested")).unwrap();

    for switches in ["/p", "/q", "/x", "/w", "/d", "/r", "/s/q", "/s^/x"] {
        let source = format!("dir {switches} {}", directory.path().display());
        let native = native_cmd(&source);
        let hidden = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["__cmd-run", "--hex", &hex_encode(source.as_bytes())])
            .output()
            .expect("hidden cmd runner should start");

        assert_eq!(hidden.status.code(), native.status.code(), "{source}");
        assert_eq!(
            dir_stdout_without_live_free_space(&hidden.stdout),
            dir_stdout_without_live_free_space(&native.stdout),
            "{source}"
        );
        assert_eq!(hidden.stderr, native.stderr, "{source}");
    }
}

#[test]
fn hidden_cmd_runner_filters_with_complete_lossless_tee_and_native_result_metadata() {
    let directory = tempdir().unwrap();
    let appdata = directory.path().join("appdata");
    let config_dir = appdata.join("rtk");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[tee]\nenabled = true\nmode = \"always\"\nmax_files = 2\nmax_file_size = 1048576\n",
    )
    .unwrap();
    let tee_dir = directory
        .path()
        .join("tee ^ %TEMP% !RTK_HINT_EXPAND! spaces");
    let tracking_db = directory.path().join("history.db");
    let environment = (0..32)
        .map(|index| {
            (
                format!("RTK_CMD_RUNNER_{index:02}"),
                format!("value-{}", "x".repeat(80)),
            )
        })
        .collect::<Vec<_>>();
    let source = "set RTK_CMD_RUNNER_";

    let native = Command::new("cmd.exe")
        .args(["/D", "/S", "/C", source])
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .output()
        .expect("native cmd.exe should start");
    let filtered = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["__cmd-run", "--hex", &hex_encode(source.as_bytes())])
        .env("APPDATA", &appdata)
        .env("RTK_TEE_DIR", &tee_dir)
        .env("RTK_DB_PATH", &tracking_db)
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .output()
        .expect("hidden cmd runner should start");

    assert_eq!(filtered.status.code(), native.status.code());
    assert_eq!(filtered.stderr, native.stderr);
    assert_ne!(
        filtered.stdout, native.stdout,
        "runner must emit compact output"
    );
    let shown = String::from_utf8(filtered.stdout).expect("structured output is UTF-8");
    assert!(shown.starts_with("[set] 32 vars:"));
    assert!(shown.ends_with("\r\n"), "filtered output keeps CMD CRLF");
    assert!(
        shown.as_bytes().iter().enumerate().all(
            |(index, byte)| *byte != b'\n' || index > 0 && shown.as_bytes()[index - 1] == b'\r'
        ),
        "filtered output must not introduce bare LF"
    );

    let tracking = rusqlite::Connection::open(&tracking_db).expect("tracking database");
    let filtered_records: i64 = tracking
        .query_row(
            "SELECT COUNT(*) FROM commands WHERE rtk_cmd = 'rtk cmd (filtered segment)'",
            [],
            |row| row.get(0),
        )
        .expect("filtered segment count");
    assert_eq!(
        filtered_records, 1,
        "only the hidden filtered segment is tracked"
    );

    let artifacts = fs::read_dir(&tee_dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "log")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        artifacts.len(),
        1,
        "accepted compact output commits one tee"
    );
    let artifact = &artifacts[0];
    assert!(shown.contains(artifact.file_name().to_string_lossy().as_ref()));
    assert_eq!(fs::read(artifact.path()).unwrap(), native.stdout);

    let hint = shown
        .trim_end_matches("\r\n")
        .strip_prefix(&shown[..shown.rfind("[full output: ").unwrap()])
        .unwrap()
        .strip_prefix("[full output: ")
        .and_then(|value| value.strip_suffix(']'))
        .expect("paste-ready CMD recovery hint");
    let mut recovered_command = Command::new("cmd.exe");
    recovered_command
        .args(["/D", "/V:ON", "/S", "/C"])
        .raw_arg(hint)
        .env("TEMP", directory.path().join("expanded-temp"))
        .env("RTK_HINT_EXPAND", "expanded-bang");
    let recovered = recovered_command
        .output()
        .expect("recovery hint should start in CMD");
    assert!(recovered.status.success(), "{hint}");
    assert_eq!(recovered.stdout, native.stdout, "{hint}");
}

fn wait_for_test_file(directory: &Path, prefix: &str) -> std::path::PathBuf {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(path) = fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
            })
        {
            return path;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {prefix} in {}",
            directory.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_test_file_count(directory: &Path, prefix: &str, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let count = fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
            .count();
        if count >= expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected} {prefix} markers in {}",
            directory.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn hidden_cmd_runner_serializes_lossless_tee_commits_between_processes() {
    let directory = tempdir().unwrap();
    let appdata = directory.path().join("appdata");
    let config_dir = appdata.join("rtk");
    let tee_dir = directory.path().join("tee");
    let hook_dir = directory.path().join("hook");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(&hook_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[tee]\nenabled = true\nmode = \"always\"\nmax_files = 1\nmax_file_size = 1048576\n",
    )
    .unwrap();

    let loaded_config = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("config")
        .env("APPDATA", &appdata)
        .output()
        .expect("rtk config should start");
    assert!(loaded_config.status.success());
    assert!(
        String::from_utf8_lossy(&loaded_config.stdout).contains("max_files = 1"),
        "children must use the isolated max_files = 1 config"
    );

    let source = "set RTK_TEE_LOCK_CHILD_";
    let child = |name: &str| {
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["__cmd-run", "--hex", &hex_encode(source.as_bytes())])
            .env("APPDATA", &appdata)
            .env("RTK_TEE_DIR", &tee_dir)
            .env("RTK_TEST_TEE_COMMIT_HOLD_DIR", &hook_dir)
            .env("RTK_TEST_TEE_COMMIT_OBSERVATION_DIR", &hook_dir)
            .env("RTK_TEST_TEE_PUBLICATION_DIR", &hook_dir)
            .env("RTK_TEST_TEE_MAX_FILES", "1")
            .envs((0..16).map(|index| {
                (
                    format!("RTK_TEE_LOCK_CHILD_{name}_{index:02}"),
                    "x".repeat(256),
                )
            }))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("hidden cmd runner should start")
    };

    let first = child("first");
    wait_for_test_file(&hook_dir, "entered-");
    let mut second = child("second");
    wait_for_test_file_count(&hook_dir, "attempting-", 2);
    let entered = fs::read_dir(&hook_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("entered-"))
        .count();
    assert_eq!(
        entered, 1,
        "only the lock holder may enter the commit critical section"
    );
    assert!(
        second.try_wait().unwrap().is_none(),
        "second process must wait for the first process's tee commit lock"
    );
    fs::write(hook_dir.join("release"), "release").unwrap();
    let first_publication = wait_for_test_file(&hook_dir, "published-");
    let first_published_path = fs::read_to_string(first_publication).unwrap();
    assert!(
        Path::new(first_published_path.trim()).is_file(),
        "the first hint target exists after stdout publication while its lock is held"
    );
    fs::write(hook_dir.join("publication-release"), "release").unwrap();

    let first_output = first.wait_with_output().unwrap();
    let second_output = second.wait_with_output().unwrap();
    for output in [&first_output, &second_output] {
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("[set] 16 vars:"));
        assert!(output.stderr.is_empty());
    }

    let observations = fs::read_dir(&hook_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("committed-"))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        observations.len(),
        2,
        "each child records its selected artifact"
    );
    let observed_paths = observations
        .into_iter()
        .map(|observation| {
            let selected = fs::read_to_string(observation).unwrap();
            let (max_files, path) = selected
                .split_once('\n')
                .expect("observation includes the active retention setting");
            assert_eq!(max_files, "1", "child must commit with max_files = 1");
            assert!(
                !path.trim().is_empty(),
                "the child-side observation is written only while the selected artifact exists"
            );
            path.to_owned()
        })
        .collect::<Vec<_>>();
    for output in [&first_output, &second_output] {
        assert!(
            observed_paths
                .iter()
                .any(|path| String::from_utf8_lossy(&output.stdout).contains(path.trim())),
            "each child returns a hint to the artifact it observed while holding the lock"
        );
    }

    let retained = fs::read_dir(&tee_dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
        .collect::<Vec<_>>();
    assert_eq!(
        retained.len(),
        1,
        "the later serialized commit retains one artifact: {retained:?}"
    );
}

#[test]
fn hidden_cmd_runner_keeps_failed_structured_query_native() {
    let source = "assoc .rtk_cmd_runner_missing_extension";
    let native = native_cmd(source);
    let hidden = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["__cmd-run", "--hex", &hex_encode(source.as_bytes())])
        .output()
        .expect("hidden cmd runner should start");

    assert!(
        !native.status.success(),
        "missing association must fail on CMD"
    );
    assert_eq!(hidden.status.code(), native.status.code());
    assert_eq!(hidden.stdout, native.stdout);
    assert_eq!(hidden.stderr, native.stderr);
}

#[test]
fn identity_builtin_output_and_binary_type_remain_native_when_captured() {
    let directory = tempdir().unwrap();
    let binary = directory.path().join("bytes.bin");
    fs::write(&binary, [0_u8, 0xff, b'\r', b'\n', b'x']).unwrap();

    assert_cmd_parity("echo exact identity output");
    assert_cmd_parity(&format!("type {}", binary.display()));
    assert_cmd_parity("cls");
    assert_cmd_parity("chdir .");
}

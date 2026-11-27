#![cfg(unix)]

mod common;

// Captured from PHPUnit 9.6.7 / PHP 8.2.36 with --colors=always.
const PHPUNIT: &str = include_str!("fixtures/phpunit_pass_raw.txt");

#[test]
fn generic_test_preserves_phpunit_success_after_earlier_stderr_notices() {
    let dir = common::tempdir();
    let capture = dir.path().join("phpunit.txt");
    std::fs::write(&capture, PHPUNIT).expect("write captured PHPUnit output");
    let script = r#"
printf '%s\n' 'cache:clear complete' 'assets:install complete' 'cache:warmup started' 'PHP Notice: setup notice' 'PHP Notice: setup notice' 'cache:warmup complete' >&2
cat "$PHPUNIT_CAPTURE"
exit "$PIPELINE_STATUS"
"#;
    for code in [0, 7] {
        let output = common::rtk_command()
            .args(["test", "sh", "-c", script])
            .env("PHPUNIT_CAPTURE", &capture)
            .env("PIPELINE_STATUS", code.to_string())
            .output()
            .expect("run generic test wrapper");
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 summary");
        if code == 0 {
            assert!(stdout.contains("OK (2 tests, 3 assertions)"), "{stdout}");
            assert!(!stdout.contains("\x1b["), "{stdout}");
            assert!(stdout.len() < PHPUNIT.len() * 8 / 10, "{stdout}");
        } else {
            assert!(stdout.contains("PHP Notice: setup notice"), "{stdout}");
            assert!(!stdout.contains("SUMMARY:"), "{stdout}");
        }
    }
}

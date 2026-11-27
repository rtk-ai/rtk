mod common;

#[cfg(unix)]
mod tests {
    use super::common;
    use std::fs;

    #[test]
    fn passing_test_output_can_be_recalled_from_the_same_execution() {
        let dir = common::tempdir();
        let raw = format!(
            "{}\nREVIEW_SIGNAL: unsafe javascript URL is accepted by the current allowlist\n1..1\n# tests 1\n# suites 0\n# pass 1\n# fail 0\n# duration_ms 1\n",
            "# successful test details\n".repeat(40)
        );
        let log = dir.path().join("test-output.txt");
        let database = dir.path().join("recall.db");
        fs::write(&log, &raw).expect("write test output");
        let output = common::rtk_command()
            .args(["test", "cat"])
            .arg(&log)
            .env_remove("RTK_TEE")
            .env("RTK_RECALL_DB", &database)
            .output()
            .expect("run passing command");
        assert!(output.status.success());
        let shown = String::from_utf8(output.stdout).expect("UTF-8 summary");
        assert!(shown.contains("# pass 1"));
        let hash = shown
            .split_once("[full output: rtk recall ")
            .expect("successful truncation must offer recovery")
            .1
            .split_once(']')
            .expect("hint terminator")
            .0;
        fs::remove_file(log).expect("remove the original output to prevent a rerun");
        let recalled = common::rtk_command()
            .args(["recall", hash])
            .env("RTK_RECALL_DB", &database)
            .output()
            .expect("recall the recorded execution");
        assert!(recalled.status.success());
        assert_eq!(recalled.stdout, raw.as_bytes());
    }
}

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

mod common;

#[test]
fn multiline_search_keeps_whole_blocks_and_a_recovery_hint() {
    let dir = common::tempdir();
    let raw = include_str!("fixtures/ast_grep_multiline_raw.txt");
    let fixture = dir.path().join("matches.txt");
    std::fs::write(&fixture, raw).expect("write matches");
    let tool = dir.path().join("ast-grep");
    std::fs::write(&tool, "#!/bin/sh\ncat \"$MATCH_FIXTURE\"\n").expect("write ast-grep fixture");
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755))
        .expect("make executable");
    let output = common::rtk_command()
        .args(["ast-grep", "run", "-p", "LazyLock::new($$$)", "blocks.rs"])
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .env("MATCH_FIXTURE", &fixture)
        .output()
        .expect("run ast-grep");
    assert!(output.status.success(), "{output:?}");
    let shown = String::from_utf8(output.stdout).expect("UTF-8 search output");
    assert_eq!(shown.matches("});").count(), 5, "{shown}");
    assert!(shown.contains("7 more match block(s)"), "{shown}");
    assert!(shown.contains("rtk recall"), "{shown}");
    assert!(shown.len() * 5 <= raw.len() * 4, "{shown}");
}

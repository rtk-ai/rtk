mod common;

#[test]
fn ls_explains_how_to_request_a_depth_limited_tree() {
    for args in [
        vec!["ls", "--depth", "3"],
        vec!["ls", "--depth=3"],
        vec!["ls", "--depth"],
    ] {
        let output = common::rtk_command().args(&args).output().expect("run ls");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("ls does not support --depth"), "{stderr}");
        assert!(stderr.contains("rtk tree -L"), "{stderr}");
        assert!(output.stdout.is_empty());
    }
}

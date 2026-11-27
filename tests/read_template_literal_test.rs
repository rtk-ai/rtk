mod common;

#[test]
fn source_filters_preserve_complete_javascript_template_values() {
    let dir = common::tempdir();
    let source = include_str!("fixtures/template_literal_source.js");
    for extension in ["js", "mjs", "cjs", "ts", "tsx"] {
        let file = dir.path().join(format!("source.{extension}"));
        std::fs::write(&file, source).unwrap();
        for level in ["minimal", "aggressive"] {
            let output = common::rtk_command()
                .args(["read", "--level", level])
                .arg(&file)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                source,
                "{extension}, {level}"
            );
        }
    }
}

#[test]
fn javascript_without_backticks_still_filters_comments() {
    let dir = common::tempdir();
    let file = dir.path().join("source.js");
    let source = format!(
        "{}const result = 42;\n",
        "// This explanation can be removed from the source preview.\n".repeat(12)
    );
    std::fs::write(&file, &source).unwrap();
    for level in ["minimal", "aggressive"] {
        let output = common::rtk_command()
            .args(["read", "--level", level])
            .arg(&file)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"const result = 42;");
        assert!(output.stdout.len() * 5 <= source.len() * 4);
    }
}

use super::*;
use serde_json::json;

fn prefix(effect: &str, root: &str, argv: &[&str]) -> Value {
    json!({"effect":effect,"durability":"local_persistent","rule":{
        "kind":"shell_command_argv_prefix","argv_prefix":argv,"workspace_root":root}})
}
fn exact(effect: &str, command: &str) -> Value {
    json!({"effect":effect,"durability":"local_persistent","rule":{
        "kind":"shell_command_exact_raw","command":command}})
}

#[test]
fn native_oracle_rows_and_untouched_only() {
    let allow = prefix("allow", "/ws", &["git", "status"]);
    let broad = prefix("allow", "/ws", &["git"]);
    let deny = prefix("deny", "/ws", &["git"]);
    let raw = exact("allow", "git status");
    let malformed_raw = json!({"effect":"allow","durability":"x","rule":{
        "kind":"shell_command_exact_raw","raw":"git status"}});
    let rooted_raw = json!({"effect":"allow","durability":"x","rule":{
        "kind":"shell_command_exact_raw","command":"git status","workspace_root":"/ws"}});
    let rootless = json!({"effect":"allow","durability":"x","rule":{
        "kind":"shell_command_argv_prefix","argv_prefix":["git","status"]}});
    let rows = vec![
        ("01", vec![], "/ws", "git status", true),
        ("02", vec![allow.clone()], "/ws", "git status", false),
        (
            "03",
            vec![prefix("allow", "/other", &["git", "status"])],
            "/ws",
            "git status",
            false,
        ),
        (
            "04",
            vec![prefix("allow", "/", &["git", "status"])],
            "/ws",
            "git status",
            false,
        ),
        ("05", vec![allow.clone()], "/ws/sub", "git status", false),
        ("06", vec![malformed_raw], "/ws", "git status", false),
        (
            "07",
            vec![deny.clone(), allow.clone()],
            "/ws",
            "git status",
            false,
        ),
        ("08", vec![broad.clone()], "/ws", "git status", false),
        (
            "09",
            vec![allow.clone()],
            "/ws",
            "git status && git log -1 --oneline",
            false,
        ),
        (
            "10",
            vec![prefix("prompt", "/ws", &["git", "status"])],
            "/ws",
            "git status",
            false,
        ),
        ("11", vec![rooted_raw.clone()], "/ws", "git status", false),
        ("12", vec![rooted_raw], "/ws", "git status", false),
        (
            "13",
            vec![allow.clone()],
            "/ws",
            "git status --short",
            false,
        ),
        ("14", vec![deny, allow.clone()], "/ws", "git status", false),
        (
            "15",
            vec![prefix("allow", "/ws/", &["git", "status"])],
            "/ws",
            "git status",
            false,
        ),
        ("16", vec![rootless], "/ws", "git status", false),
        ("17", vec![], "/ws", "git status", false),
        ("18", vec![raw.clone()], "/ws", "git status", false),
        ("21", vec![], "/ws", "git status", true),
        ("22", vec![allow.clone()], "/ws", "git status", false),
        ("23", vec![raw.clone()], "/ws", "git status", false),
        ("19", vec![raw], "/ws", "git status --short", true),
        (
            "20",
            vec![exact("deny", "git status"), allow],
            "/ws",
            "git status",
            false,
        ),
        (
            "descendant",
            vec![prefix("allow", "/ws/sub", &["git"])],
            "/ws",
            "git status",
            false,
        ),
        (
            "both allow",
            vec![broad, prefix("allow", "/ws", &["rtk"])],
            "/ws",
            "git status",
            false,
        ),
        (
            "rewrite only",
            vec![exact("allow", "rtk git status")],
            "/ws",
            "git status",
            false,
        ),
    ];
    for (id, rules, cwd, command, expected) in rows {
        let raw = json!({"schema_version":if id == "17" {3} else {2},"rules":rules});
        let parsed = serde_json::from_value::<Policy>(raw);
        let rewritten = command
            .split(" && ")
            .map(|s| format!("rtk {s}"))
            .collect::<Vec<_>>()
            .join(" && ");
        assert_eq!(
            parsed.is_ok_and(
                |p| p.schema_version == 2 && parity(&p.rules, command, &rewritten, Some(cwd))
            ),
            expected,
            "row {id}"
        );
    }
}

#[test]
fn malformed_settings_policies_and_complex_syntax_defer() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let check = || preserves_at(dir, "git status", "rtk git status", Some("/ws"));
    assert!(!check());
    fs::write(
        dir.join("settings.json"),
        r#"{"schema_version":1,"permissions":{"default_profile":":ask-me"}}"#,
    )
    .unwrap();
    assert!(check());
    for raw in [
        "broken",
        "{}",
        r#"{"schema_version":2,"rules":[],"extra":0}"#,
        r#"{"schema_version":2,"rules":[{"effect":"other","durability":"x","rule":{"kind":"other"}}]}"#,
    ] {
        fs::write(dir.join("approval-policy.json"), raw).unwrap();
        assert!(!check(), "{raw}");
    }
    let rules: Vec<Rule> = serde_json::from_value(json!([exact("allow", "unrelated")])).unwrap();
    for cmd in [
        "git 'status'",
        "git status >out",
        "X=1 git status",
        "git *",
        "env git status",
        "/bin/git status",
        "git status & git diff",
        "git $x",
        "git \\status",
        "git status\r",
    ] {
        assert!(!parity(&rules, cmd, "rtk git status", Some("/ws")), "{cmd}");
    }
    for separator in [" && ", " || ", ";", "|", "\n"] {
        assert!(parity(
            &rules,
            &format!("git status{separator}git diff"),
            &format!("rtk git status{separator}rtk git diff"),
            Some("/ws")
        ));
    }
    fs::remove_file(dir.join("approval-policy.json")).unwrap();
    for profile in [":auto-review", ":read-only", ":unrestricted", "unknown"] {
        fs::write(
            dir.join("settings.json"),
            json!({"schema_version":1,"permissions":{"default_profile":profile}}).to_string(),
        )
        .unwrap();
        assert!(!check());
    }
}

#[test]
fn strict_schema_rejects_every_missing_or_extra_field() {
    let good = json!({"schema_version":2,"rules":[prefix("allow", "/ws", &["git"])]});
    for pointer in ["", "/rules/0", "/rules/0/rule"] {
        let mut extra = good.clone();
        extra
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("future".into(), json!(true));
        assert!(serde_json::from_value::<Policy>(extra).is_err());
        for field in good.pointer(pointer).unwrap().as_object().unwrap().keys() {
            let mut missing = good.clone();
            missing
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(
                serde_json::from_value::<Policy>(missing).is_err(),
                "{pointer}/{field}"
            );
        }
    }
    for rule in [
        prefix("unknown", "/ws", &["git"]),
        json!({"effect":"allow","durability":0,"rule":{"kind":"shell_command_exact_raw","command":"git"}}),
        json!({"effect":"allow","durability":"x","rule":{"kind":"future"}}),
    ] {
        assert!(
            serde_json::from_value::<Policy>(json!({"schema_version":2,"rules":[rule]})).is_err()
        );
    }
    let rules: Vec<Rule> = serde_json::from_value(json!([
        prefix("prompt", "/ws", &["git"]),
        prefix("allow", "/ws", &["git"]),
        prefix("allow", "/ws", &["rtk"])
    ]))
    .unwrap();
    assert!(!parity(&rules, "git status", "rtk git status", Some("/ws")));
    assert!(!parity(&rules, "git status", "rtk git status", None));
    assert!(!parity(
        &rules,
        "git status",
        "rtk git status && rtk git diff",
        Some("/ws")
    ));
}

#[test]
fn review_false_parity_and_exact_raw_regressions() {
    for root in ["/ws/sub", "/", "/ws/", "/unrelated"] {
        for (a, b) in [("/ws", root), (root, "/ws")] {
            let rules: Vec<Rule> = serde_json::from_value(json!([
                prefix("allow", a, &["git"]),
                prefix("allow", b, &["rtk"])
            ]))
            .unwrap();
            for cwd in [Some("/ws"), Some("/ws/sub"), None] {
                assert!(!parity(&rules, "git status", "rtk git status", cwd));
            }
        }
    }
    for effect in ["allow", "prompt", "deny"] {
        for command in [
            "git status ",
            " git status",
            "git status\n",
            "git status && git log -3",
            "git log -3 | head",
        ] {
            let rules: Vec<Rule> = serde_json::from_value(json!([exact(effect, command)])).unwrap();
            assert!(
                !parity(&rules, command, "rtk git status", Some("/ws")),
                "{command:?}"
            );
            assert!(
                !parity(&rules, "git diff", command, Some("/ws")),
                "{command:?}"
            );
        }
    }
    let rules: Vec<Rule> = serde_json::from_value(json!([exact("allow", "git log -3")])).unwrap();
    assert!(!parity(
        &rules,
        "git status && git log -3",
        "rtk git status && rtk git log -3",
        None
    ));
    assert!(!parity(
        &rules,
        "git diff",
        "rtk git status && git log -3",
        None
    ));
}

#[test]
fn both_measured_settings_shapes_and_no_policy_commands() {
    let dir = tempfile::tempdir().unwrap();
    for version in [None, Some(json!(1)), Some(json!(2)), Some(json!(null))] {
        let mut settings = json!({"schema_version":1,"permissions":{"default_profile":":ask-me"}});
        if let Some(v) = &version {
            settings["permissions"]["schema_version"] = v.clone();
        }
        fs::write(dir.path().join("settings.json"), settings.to_string()).unwrap();
        for command in ["git status", "git status && git log -3"] {
            assert_eq!(
                preserves_at(dir.path(), command, "rtk git status", None),
                version.is_none() || version == Some(json!(1))
            );
        }
        assert!(!preserves_at(
            dir.path(),
            "git status\r",
            "rtk git status",
            None
        ));
    }
}

#[test]
fn review_wrapper_peel_set_defers() {
    let rules: Vec<Rule> =
        serde_json::from_value(json!([prefix("deny", "/ws", &["git", "push"])])).unwrap();
    for command in [
        "timeout 30 git push",
        "stdbuf -oL git push",
        "builtin git push",
        "noglob git push",
        "xargs git push",
        "nocorrect git push",
    ] {
        let rewritten = command.replace("git push", "rtk git push");
        assert!(
            !parity(&rules, command, &rewritten, Some("/ws")),
            "{command}"
        );
    }
    let rules: Vec<Rule> =
        serde_json::from_value(json!([prefix("deny", "/ws", &["git", "status"])])).unwrap();
    assert!(!parity(
        &rules,
        "timeout -s KILL 5 git status",
        "timeout -s KILL 5 rtk git status",
        Some("/ws")
    ));
}

#[test]
fn review_exact_raw_normalised_whitespace_defers() {
    for rule in ["git status", "git  status", "git\tstatus"] {
        let rules: Vec<Rule> = serde_json::from_value(json!([exact("deny", rule)])).unwrap();
        for command in ["git  status", "git\tstatus"] {
            for (original, rewritten) in [
                (command.to_string(), "rtk git status".to_string()),
                ("git diff".to_string(), command.to_string()),
                (
                    format!("git diff && {command}"),
                    "rtk git diff && rtk git status".to_string(),
                ),
                ("git diff".to_string(), format!("rtk git diff && {command}")),
            ] {
                assert!(
                    !parity(&rules, &original, &rewritten, None),
                    "{rule:?}: {original:?} -> {rewritten:?}"
                );
            }
        }
    }
    let rules: Vec<Rule> =
        serde_json::from_value(json!([exact("deny", "git status && git log -3")])).unwrap();
    assert!(!parity(
        &rules,
        "git  status && git  log -3",
        "rtk git status && rtk git log -3",
        None
    ));
    assert!(!parity(
        &rules,
        "git diff",
        "git  status && git  log -3",
        None
    ));
}

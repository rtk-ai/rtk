use super::*;

#[test]
fn literal_subset_and_strictest_decision() {
    let rules = Parser::new(
        r#"
        # literal rules, not executed
        prefix_rule(pattern = ['git', ['status', 'diff',],], justification = "read only",)
        prefix_rule(decision = "prompt", pattern = ['git'])
        prefix_rule(pattern = ['git', 'diff'], decision = "forbidden")
    "#,
    )
    .rules()
    .expect("literal rules");
    assert_eq!(
        evaluate(&shell_split("git status"), &rules),
        Some(Decision::Prompt)
    );
    assert_eq!(
        evaluate(&shell_split("git diff HEAD"), &rules),
        Some(Decision::Forbidden)
    );
    assert_eq!(evaluate(&shell_split("rtk git status"), &rules), None);
}

#[test]
fn unsupported_or_malformed_syntax_defers_whole_file() {
    for text in [
        "load('policy.star', 'rules')",
        "prefix_rule(pattern=commands)",
        "prefix_rule(pattern=[])",
        "prefix_rule(pattern=[[]])",
        "prefix_rule(pattern=['git'], decision='deny')",
        "prefix_rule(pattern=['git'], decision='prompt-user')",
        "prefix_rule(pattern=['git'], justification='')",
        "prefix_rule(pattern=['git'], justification='  \t ')",
        "prefix_rule(pattern=['git'], decision='allow', decision='forbidden')",
        "prefix_rule(pattern=['git'], match=['git status'])",
        "prefix_rule(pattern=['git'], unknown=True)",
        "prefix_rule(pattern=['git'] + ['status'])",
        "prefix_rule(pattern=['git']) ; exit()",
        "prefix_rule(pattern=['git'])\nprefix_rule(pattern=[computed])",
        "host_executable(name='git', paths=['/usr/bin/git'])",
        "prefix_rule(pattern=['git']",
        "prefix_rule(pattern=['git' 'status'])",
        "prefix_rule(pattern=['git']) prefix_rule(pattern=['rtk'])",
        "prefix_rule\n(pattern=['git'])",
        r#"prefix_rule(pattern=["g\x69t"])"#,
    ] {
        assert!(Parser::new(text).rules().is_none(), "{text}");
    }
}

#[test]
fn decisions_are_per_segment_and_defer_shell_expansion() {
    let rules = Parser::new("prefix_rule(pattern=['git'])")
        .rules()
        .expect("rule");
    assert_eq!(
        decisions("git status && git diff", &rules),
        Some(vec![Some(Decision::Allow); 2])
    );
    for cmd in [
        "$TOOL status",
        "/usr/bin/git status",
        "X=y git status",
        "git diff *",
        "git status > out",
        "env git status",
        "command git status",
        "bash -c 'git status'",
    ] {
        assert!(decisions(cmd, &rules).is_none(), "{cmd}");
    }
}

#[test]
fn system_and_managed_layers_defer() {
    use crate::core::{test_isolation, user_env};
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let cwd = temp.path().join("project");
        fs::create_dir_all(&cwd).expect("project");
        let _entered = test_isolation::enter(&cwd);
        let home = temp.path().join("codex");
        fs::create_dir_all(&home).expect("home");
        user_env::with_path("CODEX_HOME", Some(&home), || {
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::create_dir_all(home.join("rules")).expect("rules");
            fs::write(home.join("rules/a.rules"), "prefix_rule(pattern=['git'])").expect("rule");
            assert!(!preserves_rules(
                None,
                "wrapper git status",
                "wrapper rtk git status"
            ));
            fs::remove_file(home.join("rules/a.rules")).expect("remove rule fixture");
            fs::write(home.join("managed_config.toml"), "").expect("managed");
            assert!(!preserves_rules(None, "git status", "rtk git status"));
            fs::remove_file(home.join("managed_config.toml")).expect("remove fixture");
            fs::write(home.join("config.toml"), "model = 'example'").expect("plain config");
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::write(home.join("config.toml"), "profile = 'restricted'").expect("profile");
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::remove_file(home.join("config.toml")).expect("remove config fixture");
            let system = user_dirs::codex_system_config_dir();
            fs::create_dir_all(&system).expect("system");
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::create_dir_all(system.join("rules")).expect("system rules");
            fs::write(system.join("rules/a.rules"), "prefix_rule(pattern=['git'])").expect("rule");
            assert!(!preserves_rules(None, "git status", "rtk git status"));
        });
    });
}

#[test]
fn continuations_and_complex_words_defer() {
    let rules = Parser::new("prefix_rule(pattern=['git', 'push'], decision='forbidden')")
        .rules()
        .expect("rule");
    for cmd in [
        "git \\\npush",
        "git status && git \\\npush",
        "git status; \\\ngit push",
        "git status | \\\ngit push",
        "\\\ngit push",
        "git status && \\\ngit push --force",
        "git status && \\\ngit push",
        "git 'status'",
        "git \"status\"",
        "git diff [ab]",
        "git diff {a,b}",
        "git diff $REV",
        "git status > out",
        "X=y git status",
        "nice git status",
        "time git status",
        "sudo git status",
        "/usr/bin/git status",
        "git status & git diff",
        "(git status)",
        "git status # comment",
        "! git status",
    ] {
        assert!(decisions(cmd, &rules).is_none(), "{cmd:?}");
    }
}

#[test]
fn no_rules_fast_path_and_ancestor_project_rules() {
    use crate::core::{test_isolation, user_env};
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let cwd = temp.path().join("project/sub");
        fs::create_dir_all(&cwd).expect("project");
        fs::create_dir_all(temp.path().join("project/.git")).expect("root");
        fs::create_dir_all(temp.path().join(".codex/rules")).expect("ancestor");
        let _entered = test_isolation::enter(&cwd);
        let home = cwd.join("relative");
        fs::create_dir_all(home.join("rules")).expect("home");
        user_env::with_path("CODEX_HOME", Some(Path::new("relative")), || {
            for config in [
                "[features]\nhooks=true",
                "profile='p'\n[profiles.p]",
                "project_root_markers=['.git']",
                "invalid TOML",
            ] {
                fs::write(home.join("config.toml"), config).expect("config");
                assert!(preserves_rules(None, "git 'status'", "rtk git status"));
            }
            fs::write(
                temp.path().join(".codex/rules/a.rules"),
                "prefix_rule(pattern=['git', 'push'], decision='forbidden')",
            )
            .expect("ancestor rule above HEAD-less git directory");
            assert!(!preserves_rules(None, "git push", "rtk git push"));
            fs::remove_file(temp.path().join(".codex/rules/a.rules"))
                .expect("remove ancestor rule");
            assert!(preserves_rules(None, "git push", "rtk git push"));
            let project = temp.path().join("project/.codex");
            fs::create_dir_all(project.join("rules")).expect("project rules");
            fs::write(project.join("config.toml"), "[features]").expect("project config");
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::write(
                project.join("rules/a.rules"),
                "prefix_rule(pattern=['git'])",
            )
            .expect("project rule");
            assert!(!preserves_rules(None, "git status", "rtk git status"));
            fs::remove_file(project.join("rules/a.rules")).expect("remove project rule");
            fs::write(home.join("rules/a.rules"), "prefix_rule(pattern=['git'])")
                .expect("user rule");
            assert!(!preserves_rules(None, "git status", "rtk git status"));
        });
    });
}

#[cfg(unix)]
#[test]
fn symlinked_rules_directory_follows_but_rule_entries_defer() {
    use crate::core::{test_isolation, user_env};
    use std::os::unix::fs::symlink;
    let temp = test_isolation::tempdir();
    test_isolation::with_root(temp.path(), || {
        let cwd = temp.path().join("project");
        fs::create_dir_all(&cwd).expect("project");
        let _entered = test_isolation::enter(&cwd);
        let home = temp.path().join("codex");
        let real = temp.path().join("real-rules");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(&real).expect("rules");
        symlink(&real, home.join("rules")).expect("directory symlink");
        user_env::with_path("CODEX_HOME", Some(&home), || {
            assert!(preserves_rules(None, "git status", "rtk git status"));
            fs::write(
                real.join("a.rules"),
                "prefix_rule(pattern=[['git', 'rtk']])",
            )
            .expect("allow rule");
            assert!(preserves_rules(None, "git status", "rtk git status"));
            symlink(real.join("a.rules"), real.join("b.rules")).expect("entry symlink");
            assert!(!preserves_rules(None, "git status", "rtk git status"));
        });
    });
}

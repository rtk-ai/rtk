use std::path::Path;
use std::process::Command;

fn find(cwd: &Path, args: &[&str]) -> String {
    let state = tempfile::tempdir().expect("temporary state directory");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("find")
        .args(args)
        .current_dir(cwd)
        .env("RTK_TEE_DIR", state.path())
        .env("RTK_DB_PATH", state.path().join("tracking.db"))
        .env("RTK_RECALL_DB", state.path().join("recall.db"))
        .output()
        .expect("run rtk find");
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

#[test]
fn native_and_compact_syntax_preserve_directory_roots_and_spaced_paths() {
    let tmp = tempfile::tempdir().expect("temporary search directory");
    let relative = Path::new("search root")
        .join("a long directory name that makes the prefix exceed fifty characters")
        .join("日本語");
    std::fs::create_dir_all(tmp.path().join(&relative)).expect("create nested directory");
    let expected: Vec<_> = (0..12)
        .map(|i| {
            let path = relative.join(format!("file {i:02}.txt"));
            std::fs::write(tmp.path().join(&path), "test").expect("create file");
            path.to_string_lossy().into_owned()
        })
        .collect();
    for root in [
        "search root".to_string(),
        tmp.path()
            .join("search root")
            .to_string_lossy()
            .into_owned(),
    ] {
        for args in [
            vec![root.as_str(), "-name", "*.txt", "-type", "f"],
            vec!["*.txt", root.as_str()],
        ] {
            let shown = find(tmp.path(), &args);
            let wanted: Vec<_> = expected
                .iter()
                .map(|p| {
                    if Path::new(&root).is_absolute() {
                        tmp.path().join(p).to_string_lossy().into_owned()
                    } else {
                        p.clone()
                    }
                })
                .collect();
            assert_eq!(shown, format!("{}\n", wanted.join("\n")));
            assert!(shown.lines().all(|line| tmp.path().join(line).is_file()));
        }
    }
}

#[test]
fn single_file_root_and_empty_search_keep_native_output() {
    let tmp = tempfile::tempdir().expect("temporary search directory");
    std::fs::create_dir(tmp.path().join("root")).expect("create root");
    std::fs::write(tmp.path().join("root/file name.txt"), "test").expect("create file");
    assert_eq!(
        find(tmp.path(), &["root/file name.txt", "-type", "f"]),
        "root/file name.txt\n"
    );
    assert_eq!(find(tmp.path(), &["*.missing", "root"]), "");
}

#[test]
fn restoring_root_keeps_explicit_cap_selection() {
    let tmp = tempfile::tempdir().expect("temporary search directory");
    std::fs::create_dir_all(tmp.path().join("root/!dir")).expect("create directory");
    std::fs::write(tmp.path().join("root/!dir/a.txt"), "test").expect("create nested file");
    std::fs::write(tmp.path().join("root/z.txt"), "test").expect("create root file");
    let first = Path::new("root").join("!dir").join("a.txt");
    assert_eq!(
        find(tmp.path(), &["*.txt", "root", "-m", "1"]),
        format!("{}\n+1 more\n", first.display())
    );
    assert_eq!(
        find(tmp.path(), &["*.txt", "root", "-m", "0"]),
        "\n+2 more\n"
    );
}

#[test]
fn default_cap_recovers_complete_paths_in_display_order() {
    let tmp = tempfile::tempdir().expect("temporary search directory");
    let state = tempfile::tempdir().expect("temporary state directory");
    std::fs::create_dir_all(state.path().join("rtk")).expect("create config directory");
    std::fs::write(
        state.path().join("rtk/config.toml"),
        "[retriever]\nmode = \"tee\"\n",
    )
    .expect("write recovery config");
    let mut expected = Vec::new();
    for dir in ["logs", "logs-old"] {
        std::fs::create_dir_all(tmp.path().join("root").join(dir)).expect("create directory");
        for i in 0..30 {
            let path = Path::new("root").join(dir).join(format!("file {i:02}.txt"));
            std::fs::write(tmp.path().join(&path), "test").expect("create file");
            expected.push(path.to_string_lossy().into_owned());
        }
    }
    let tee = state.path().join("tee");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["find", "*.txt", "root"])
        .current_dir(tmp.path())
        .env("APPDATA", state.path())
        .env("XDG_CONFIG_HOME", state.path())
        .env("RTK_DB_PATH", state.path().join("tracking.db"))
        .env("RTK_RECALL_DB", state.path().join("recall.db"))
        .env("RTK_TEE_DIR", &tee)
        .env_remove("RTK_RECALL")
        .env_remove("RTK_TEE")
        .output()
        .expect("run capped find");
    assert!(output.status.success(), "{output:?}");
    let shown = String::from_utf8(output.stdout).expect("UTF-8 output");
    assert!(shown.starts_with(&format!("{}\n+10 more\n", expected[..50].join("\n"))));
    if let Some((_, rest)) = shown.split_once("rtk recall ") {
        let hash = rest.split(']').next().expect("recall hash");
        let recalled = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["recall", hash])
            .env("RTK_RECALL_DB", state.path().join("recall.db"))
            .output()
            .expect("recall hidden paths");
        assert!(recalled.status.success(), "{recalled:?}");
        assert_eq!(
            String::from_utf8(recalled.stdout).expect("UTF-8 recalled paths"),
            expected[50..].join("\n")
        );
        return;
    }
    assert!(shown.contains("tail -n +51"), "{shown}");
    let logs: Vec<_> = std::fs::read_dir(&tee)
        .expect("recovery directory")
        .map(|entry| entry.expect("recovery entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert_eq!(logs.len(), 1);
    let recovered = std::fs::read_to_string(&logs[0]).expect("read recovered paths");
    assert_eq!(recovered.lines().collect::<Vec<_>>(), expected);
}

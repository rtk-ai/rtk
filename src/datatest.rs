use std::collections::BTreeMap;
use std::fs;
use std::panic;
use std::path::{Path, PathBuf};

trait PathExt {
    fn file_name_str(&self) -> &str;
    fn file_stem_str(&self) -> &str;
    fn extension_str(&self) -> &str;
}

impl PathExt for Path {
    fn file_name_str(&self) -> &str {
        self.file_name()
            .expect("path has no file name")
            .to_str()
            .expect("non-UTF-8 path")
    }
    fn file_stem_str(&self) -> &str {
        self.file_stem()
            .expect("path has no stem")
            .to_str()
            .expect("non-UTF-8 path")
    }
    fn extension_str(&self) -> &str {
        self.extension()
            .expect("path has no extension")
            .to_str()
            .expect("non-UTF-8 path")
    }
}

pub(crate) struct Case {
    pub(crate) input: String,
    pub(crate) expected: String,
    /// The segment between `.after.` and the final extension.
    /// `None` for files named `<name>.after.<ext>` with no params segment.
    pub(crate) params: Option<String>,
    pub(crate) input_path: PathBuf,
    pub(crate) expected_path: PathBuf,
}

/// Runs test cases found under `tests/data/`.
///
/// **Path syntax**:
/// - `"subdir/*"` — run all groups in `tests/data/subdir/`
/// - `"subdir/prefix"` — run only groups whose name starts with `prefix`
///   in `tests/data/subdir/` (group = prefix + extension, e.g. `language.rs`)
///
/// Each input file `<name>.<ext>` is paired with one or more expected files:
///   `<name>.after.<ext>`               — no params
///   `<name>.after.<params>.<ext>`      — with params
///
/// The closure returns the actual output; the engine compares it against the
/// expected file (both sides trimmed).
///
/// **Update mode**: set `RTK_UPDATE_TEST_DATA=1` to overwrite expected files
/// with the actual output instead of failing.  Re-run without the variable to
/// confirm everything is green.
///
/// All failures are collected and reported together at the end.
pub(crate) fn run_tests<F>(path: &str, test_fn: F)
where
    F: Fn(&Case) -> String + panic::RefUnwindSafe,
{
    let (dir_str, prefix_filter) = match path.strip_suffix("/*") {
        Some(dir) => (dir, None),
        None => match path.rfind('/') {
            Some(i) => (&path[..i], Some(&path[i + 1..])),
            None => (path, None),
        },
    };

    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(dir_str);

    let cases = discover(&dir, prefix_filter);
    assert!(
        !cases.is_empty(),
        "no test cases found in {}{}",
        dir.display(),
        prefix_filter
            .map(|p| format!(" (prefix: {p})"))
            .unwrap_or_default()
    );

    let update = std::env::var("RTK_UPDATE_TEST_DATA").is_ok_and(|v| !v.is_empty());

    let mut failures = Vec::new();
    let mut updated = Vec::new();

    for case in &cases {
        let file_name = case.input_path.file_name_str();
        let label = match &case.params {
            Some(p) => format!("{file_name} [{p}]"),
            None => file_name.to_string(),
        };

        let prev_hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let result = panic::catch_unwind(|| test_fn(case));
        panic::set_hook(prev_hook);

        match result {
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| e.downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                failures.push(format!("FAILED {label}:\n{msg}"));
            }
            Ok(actual) => {
                if actual.trim() != case.expected.trim() {
                    if update {
                        let mut content = actual.trim().to_string();
                        content.push('\n');
                        fs::write(&case.expected_path, &content).unwrap_or_else(|e| {
                            panic!("write {}: {}", case.expected_path.display(), e)
                        });
                        updated.push(label);
                    } else {
                        failures.push(format!(
                            "FAILED {label}:\n--- expected\n{}\n+++ actual\n{}",
                            case.expected.trim(),
                            actual.trim()
                        ));
                    }
                }
            }
        }
    }

    if !updated.is_empty() {
        panic!(
            "{} expected file(s) updated — re-run without RTK_UPDATE_TEST_DATA to verify:\n  {}",
            updated.len(),
            updated.join("\n  ")
        );
    }

    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n\n{}\n\n(set RTK_UPDATE_TEST_DATA=1 to overwrite)",
        failures.len(),
        failures.join("\n\n")
    );
}

fn discover(dir: &Path, prefix_filter: Option<&str>) -> Vec<Case> {
    // Group files by the name prefix before the first dot.
    // With a prefix filter, only the matching group is processed;
    // within that group each distinct extension forms its own input+expected set.
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read dir {}: {}", dir.display(), e)) {
        let path = entry
            .unwrap_or_else(|e| panic!("read dir entry in {}: {}", dir.display(), e))
            .path();
        if !path.is_file() || path.extension().is_none() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let prefix = name.split('.').next().unwrap_or(name).to_string();
            if prefix_filter.is_none_or(|f| prefix == f) {
                groups.entry(prefix).or_default().push(path);
            }
        }
    }

    let mut cases = Vec::new();

    for (prefix, mut paths) in groups {
        paths.sort();

        let inputs: Vec<&PathBuf> = paths
            .iter()
            .filter(|p| {
                !p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains(".after."))
                    .unwrap_or(false)
            })
            .collect();

        if inputs.is_empty() {
            panic!(
                "no input file for group '{}' in {} (found only .after.* files)",
                prefix,
                dir.display()
            );
        }

        for input_path in inputs {
            let ext = input_path.extension_str().to_string();
            let stem = input_path.file_stem_str().to_string();
            let input = fs::read_to_string(input_path)
                .unwrap_or_else(|e| panic!("read {}: {}", input_path.display(), e));

            let after_prefix = format!("{}.after.", stem);
            let ext_suffix = format!(".{}", ext);

            for expected_path in paths.iter().filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&after_prefix) && n.ends_with(&ext_suffix))
                    .unwrap_or(false)
            }) {
                let expected_name = expected_path.file_name_str();
                let middle = expected_name
                    .strip_prefix(&after_prefix)
                    .and_then(|s| s.strip_suffix(&ext_suffix))
                    .unwrap_or("");
                let params = (!middle.is_empty()).then(|| middle.to_string());

                let expected = fs::read_to_string(expected_path)
                    .unwrap_or_else(|e| panic!("read {}: {}", expected_path.display(), e));

                cases.push(Case {
                    input: input.clone(),
                    expected,
                    params,
                    input_path: input_path.clone(),
                    expected_path: expected_path.clone(),
                });
            }
        }
    }

    cases
}

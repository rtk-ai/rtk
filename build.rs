use std::collections::HashSet;
use std::fs;
use std::path::Path;

fn main() {
    #[cfg(windows)]
    {
        // Clap + the full command graph can exceed the default 1 MiB Windows
        // main-thread stack during process startup. Reserve a larger stack for
        // the CLI binary so `rtk.exe --version`, `--help`, and hook entry
        // points start reliably without requiring ad-hoc RUSTFLAGS.
        println!("cargo:rustc-link-arg=/STACK:8388608");
    }

    check_cmds_modules_declared();

    let filters_dir = Path::new("src/filters");
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo");
    let dest = Path::new(&out_dir).join("builtin_filters.toml");

    // Rebuild when any file in src/filters/ changes
    println!("cargo:rerun-if-changed=src/filters");

    let mut files: Vec<_> = fs::read_dir(filters_dir)
        .expect("src/filters/ directory must exist")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();

    // Sort alphabetically for deterministic filter ordering
    files.sort_by_key(|e| e.file_name());

    let mut combined = String::from("schema_version = 1\n\n");

    for entry in &files {
        let content = fs::read_to_string(entry.path())
            .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", entry.path(), e));
        combined.push_str(&format!(
            "# --- {} ---\n",
            entry.file_name().to_string_lossy()
        ));
        combined.push_str(&content);
        combined.push_str("\n\n");
    }

    // Validate: parse the combined TOML to catch errors at build time
    let parsed: toml::Value = combined.parse().unwrap_or_else(|e| {
        panic!(
            "TOML validation failed for combined filters:\n{}\n\nCheck src/filters/*.toml files",
            e
        )
    });

    // Detect duplicate filter names across files
    if let Some(filters) = parsed.get("filters").and_then(|f| f.as_table()) {
        let mut seen: HashSet<String> = HashSet::new();
        for key in filters.keys() {
            if !seen.insert(key.clone()) {
                panic!(
                    "Duplicate filter name '{}' found across src/filters/*.toml files",
                    key
                );
            }
        }
    }

    fs::write(&dest, combined).expect("Failed to write combined builtin_filters.toml");
}

/// Every `.rs` under `src/cmds/<ecosystem>/` must be declared in that folder's
/// `mod.rs`. An undeclared file is not part of the crate: it is never compiled,
/// never linted, and its tests never run, all without any diagnostic.
fn check_cmds_modules_declared() {
    println!("cargo:rerun-if-changed=src/cmds");

    let cmds = Path::new("src/cmds");
    let ecosystems = fs::read_dir(cmds).expect("src/cmds/ directory must exist");

    for ecosystem in ecosystems {
        let dir = ecosystem.expect("failed to read src/cmds/ entry").path();
        let mod_rs = dir.join("mod.rs");
        if !dir.is_dir() || !mod_rs.exists() {
            continue;
        }

        let declared = fs::read_to_string(&mod_rs)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", mod_rs.display(), e));

        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", dir.display(), e));

        for entry in entries {
            let path = entry.expect("failed to read directory entry").path();
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let stem = path
                .file_stem()
                .expect("a .rs path always has a stem")
                .to_string_lossy()
                .to_string();
            if stem == "mod" {
                continue;
            }
            if !declared.contains(&format!("mod {stem};")) {
                panic!(
                    "{} is not declared in {}.\n\
                     Add `pub mod {};` there, keeping the list alphabetical.\n\
                     An undeclared module is never compiled, linted, or tested.",
                    path.display(),
                    mod_rs.display(),
                    stem
                );
            }
        }
    }
}

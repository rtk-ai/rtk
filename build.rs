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

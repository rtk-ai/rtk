use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Reserve a larger main-thread stack for Windows targets.
///
/// Clap + the full command graph can exceed the default 1 MiB Windows stack
/// during process startup, so `rtk.exe --version`, `--help`, and the hook entry
/// points need the headroom to start reliably without ad-hoc RUSTFLAGS.
///
/// Two things here are deliberately keyed off the **target**, not the host:
/// `CARGO_CFG_TARGET_OS` (a `#[cfg(windows)]` here would describe the machine
/// running the build, so cross-compiling to Windows from Linux would silently
/// skip the flag), and the flag syntax itself — `link.exe` takes `/STACK:`
/// while the MinGW `ld` behind `x86_64-pc-windows-gnu` only understands
/// `--stack`, and feeding it `/STACK:` makes it look for a file by that name.
fn set_windows_stack_size() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    const STACK_BYTES: usize = 8 * 1024 * 1024;
    let arg = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("gnu") => format!("-Wl,--stack,{STACK_BYTES}"),
        _ => format!("/STACK:{STACK_BYTES}"),
    };
    println!("cargo:rustc-link-arg={arg}");
}

fn main() {
    set_windows_stack_size();

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

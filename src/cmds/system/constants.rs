use std::path::{Component, Path};

pub const NOISE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "__pycache__",
    ".next",
    "dist",
    "build",
    ".cache",
    ".turbo",
    ".vercel",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".venv",
    "venv",
    "env", // Python legacy virtualenv dir — noise. .env (dotenv) is intentionally NOT here: agents must see it.
    "coverage",
    ".nyc_output",
    ".DS_Store",
    "Thumbs.db",
    ".idea",
    ".vscode",
    ".vs",
    "*.egg-info",
    ".eggs",
];

/// Storage metadata that should never appear in compact filesystem discovery.
pub const SYSTEM_METADATA_PATTERNS: &[&str] = &["._*", "@eaDir"];

pub fn is_system_metadata_name(name: &str) -> bool {
    name == "@eaDir" || name.starts_with("._")
}

pub fn is_system_metadata_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => is_system_metadata_name(&name.to_string_lossy()),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_appledouble_and_synology_metadata() {
        assert!(is_system_metadata_name("._index.ts"));
        assert!(is_system_metadata_name("@eaDir"));
        assert!(is_system_metadata_path(Path::new(
            "src/@eaDir/index.ts/SYNOPHOTO_THUMB_XL.jpg"
        )));
        assert!(is_system_metadata_path(Path::new("src/._server.ts")));
    }

    #[test]
    fn preserves_real_project_paths() {
        assert!(!is_system_metadata_name(".env"));
        assert!(!is_system_metadata_name("eaDir"));
        assert!(!is_system_metadata_path(Path::new("src/index.ts")));
        assert!(!is_system_metadata_path(Path::new("src/.config/app.toml")));
    }
}

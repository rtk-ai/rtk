//! Hard deny-list for credential/secret files (#2817).
//!
//! `.gitignore` is not a security boundary: `rtk grep` shells out to the system
//! grep, which happily prints `.env` files and private keys straight into agent
//! transcripts. This module provides a single, engine-independent exclude list
//! applied by every filesystem-walking subcommand (`grep`/`rg`, `find`, `read`).
//! It is unconditional by design — it does not depend on being inside a git
//! repo or on `.gitignore` being correct — and can only be lifted explicitly
//! with `--include-secrets`.
//!
//! Matching is case-insensitive and, because a filename-only check is
//! bypassable via an innocuously-named symlink, symlinks are also matched
//! against their resolved target.

use std::path::{Component, Path};

/// CLI flag that lifts the exclusion. Parsed manually by subcommands whose
/// args are `trailing_var_arg` (grep/rg/find); a regular clap flag on `read`.
pub const INCLUDE_SECRETS_FLAG: &str = "--include-secrets";

/// File names that are credentials wherever they appear.
const EXACT_NAMES: &[&str] = &[".env", ".git-credentials", ".pgpass", ".dockercfg"];

/// File-name prefixes: `.env.local`, `id_rsa.pub`, `.netrc.bak`, …
const NAME_PREFIXES: &[&str] = &[
    ".env.",
    "id_rsa",
    "id_ed25519",
    "id_dsa",
    "id_ecdsa",
    ".npmrc",
    ".pypirc",
    ".netrc",
];

/// File-name suffixes (extensions) that denote key material or state
/// containing credentials.
const NAME_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".kdbx", ".tfstate", ".tfvars"];

/// Directory components whose entire subtree is credential-bearing. Only
/// unambiguous dotted config dirs belong here — a generic name like `secrets`
/// would hide legitimate source modules (e.g. `src/secrets/`); credential
/// *files* inside such dirs are still caught by the name rules.
const SECRET_DIRS: &[&str] = &[".aws", ".azure", ".kube", ".docker"];

/// Adjacent (parent, child) directory pairs — precise enough to avoid
/// excluding e.g. a source directory that merely happens to be named `gcloud`.
const SECRET_DIR_PAIRS: &[(&str, &str)] = &[(".config", "gcloud")];

/// Case-insensitive check of a bare file name against the deny tables.
fn name_is_secret(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    EXACT_NAMES.contains(&name.as_str())
        || NAME_PREFIXES.iter().any(|p| name.starts_with(p))
        || NAME_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Check path components (directories) against the deny tables. The final
/// component is a file/entry name judged separately (`name_is_secret` /
/// `is_secret_dir_name`), so only its ancestors count as directories here.
fn components_are_secret(path: &Path) -> bool {
    let comps: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let Some((_, ancestors)) = comps.split_last() else {
        return false;
    };

    ancestors.iter().any(|c| SECRET_DIRS.contains(&c.as_str()))
        || comps
            .windows(2)
            .any(|w| SECRET_DIR_PAIRS.contains(&(w[0].as_str(), w[1].as_str())))
}

/// Literal (non-symlink-resolving) check.
fn literal_is_secret(path: &Path) -> bool {
    let name_hit = path
        .file_name()
        .map(|n| name_is_secret(&n.to_string_lossy()))
        .unwrap_or(false);
    name_hit || components_are_secret(path)
}

/// True when `name` matches a deny-listed directory name. Used by walkers to
/// prune descent into e.g. `.aws/` or `.kube/`.
pub fn is_secret_dir_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SECRET_DIRS.contains(&name.as_str())
}

/// Walker-oriented check: `true` for deny-listed directories (so walkers can
/// prune descent) and for secret files (symlink-target aware).
pub fn is_secret_entry(path: &Path, is_dir: bool) -> bool {
    if is_dir {
        path.file_name()
            .map(|n| is_secret_dir_name(&n.to_string_lossy()))
            .unwrap_or(false)
            || components_are_secret(path)
    } else {
        is_secret_path(path)
    }
}

/// True when `path` denotes a credential/secret file, either literally or —
/// when it is a symlink — via its resolved target (#2817: an innocuously-named
/// symlink pointing at a secret file must not bypass the exclusion).
pub fn is_secret_path(path: &Path) -> bool {
    if literal_is_secret(path) {
        return true;
    }
    match std::fs::symlink_metadata(path) {
        Ok(md) if md.file_type().is_symlink() => std::fs::canonicalize(path)
            .map(|target| literal_is_secret(&target))
            .unwrap_or(false),
        _ => false,
    }
}

/// `--exclude`/`--exclude-dir` flags for a system-grep passthrough run.
/// fnmatch is case-sensitive, so this is best-effort; the grouping path
/// re-checks results with `is_secret_path`.
pub fn grep_exclude_flags() -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    for n in EXACT_NAMES {
        flags.push(format!("--exclude={}", n));
    }
    for p in NAME_PREFIXES {
        flags.push(format!("--exclude={}*", p));
    }
    for s in NAME_SUFFIXES {
        flags.push(format!("--exclude=*{}", s));
    }
    for d in SECRET_DIRS {
        flags.push(format!("--exclude-dir={}", d));
    }
    flags
}

/// `--iglob` exclusion flags for a ripgrep passthrough run (case-insensitive).
pub fn rg_exclude_flags() -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    for n in EXACT_NAMES {
        flags.push(format!("--iglob=!**/{}", n));
    }
    for p in NAME_PREFIXES {
        flags.push(format!("--iglob=!**/{}*", p));
    }
    for s in NAME_SUFFIXES {
        flags.push(format!("--iglob=!**/*{}", s));
    }
    for d in SECRET_DIRS {
        flags.push(format!("--iglob=!**/{}/**", d));
    }
    for (a, b) in SECRET_DIR_PAIRS {
        flags.push(format!("--iglob=!**/{}/{}/**", a, b));
    }
    flags
}

/// Removes `--include-secrets` from `args` (only before a bare `--`, after
/// which everything is positional). Returns the cleaned args and whether the
/// flag was present.
pub fn strip_include_secrets(args: &[String]) -> (Vec<String>, bool) {
    let mut found = false;
    let mut out = Vec::with_capacity(args.len());
    let mut positional_only = false;
    for a in args {
        if a == "--" {
            positional_only = true;
        }
        if !positional_only && a == INCLUDE_SECRETS_FLAG {
            found = true;
            continue;
        }
        out.push(a.clone());
    }
    (out, found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn secret(p: &str) -> bool {
        is_secret_path(&PathBuf::from(p))
    }

    #[test]
    fn exact_names() {
        assert!(secret(".env"));
        assert!(secret("app/.env"));
        assert!(secret("./.git-credentials"));
        assert!(secret(".pgpass"));
        assert!(!secret("environment.rs"));
        assert!(!secret("env.rs"));
    }

    #[test]
    fn prefixes() {
        assert!(secret(".env.local"));
        assert!(secret(".env.production"));
        assert!(secret("id_rsa"));
        assert!(secret("keys/id_rsa.pub"));
        assert!(secret("id_ed25519"));
        assert!(secret(".npmrc"));
        assert!(secret(".netrc.bak"));
        assert!(!secret("id_card.rs"));
    }

    #[test]
    fn suffixes() {
        assert!(secret("server.pem"));
        assert!(secret("certs/tls.key"));
        assert!(secret("vault.kdbx"));
        assert!(secret("infra/terraform.tfstate"));
        assert!(secret("prod.tfvars"));
        assert!(!secret("monkey.rs"));
        assert!(!secret("keyboard.ts"));
    }

    #[test]
    fn case_insensitive() {
        assert!(secret(".ENV"));
        assert!(secret("SECRET.PEM"));
        assert!(secret("ID_RSA"));
    }

    #[test]
    fn secret_directories() {
        assert!(secret("/home/u/.aws/credentials"));
        assert!(secret("/home/u/.aws/config"));
        assert!(secret(".kube/config"));
        assert!(secret("/home/u/.config/gcloud/access_tokens.db"));
        assert!(secret("/home/u/.docker/config.json"));
        // Generic `secrets` dirs are NOT excluded (would hide source modules);
        // credential files inside them are still caught by name rules.
        assert!(!secret("deploy/secrets/notes.txt"));
        assert!(secret("deploy/secrets/token.pem"));
        // Unrelated dirs stay searchable.
        assert!(!secret("src/config/mod.rs"));
        assert!(!secret("gcloud/main.rs"));
    }

    #[test]
    fn strip_flag() {
        let args: Vec<String> = ["-rn", "--include-secrets", "PAT", "."]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (cleaned, found) = strip_include_secrets(&args);
        assert!(found);
        assert_eq!(cleaned, vec!["-rn", "PAT", "."]);

        // After `--` the token is positional and must survive.
        let args: Vec<String> = ["PAT", "--", "--include-secrets"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (cleaned, found) = strip_include_secrets(&args);
        assert!(!found);
        assert_eq!(cleaned, vec!["PAT", "--", "--include-secrets"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_target_is_checked() {
        let dir = std::env::temp_dir().join(format!("rtk_secrets_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let target = dir.join("secret.pem");
        std::fs::write(&target, "FAKE").expect("write target");
        let link = dir.join("innocuous.txt");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        assert!(is_secret_path(&link), "symlink to secret must be excluded");
        assert!(!is_secret_path(&dir.join("plain.txt")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

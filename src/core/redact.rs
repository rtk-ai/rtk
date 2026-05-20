//! Shared credential/PII redactor used by tracking and tee.
//!
//! `redact_command` scrubs known credential shapes from a command line
//! (URL userinfo, Bearer / Authorization headers, `--token=`/`--password=`/etc,
//! inline `FOO_TOKEN=…` assignments, and well-known credential prefixes such
//! as `ghp_`, `sk-`, `AKIA…`). `redact_project_path` reduces a filesystem
//! path to `<basename>#<8-hex-sha256>` so the tracking DB does not pin a
//! user's private layout.
//!
//! All regex are `lazy_static!`-cached and the public functions perform a
//! single pass per call — they run on the `Tracker::record` hot path.
//!
//! `redact_content` reuses the same regex bundle for free-form command output
//! that may contain credential-shaped substrings (used by the tee write path,
//! see the security & privacy design doc). It returns both the redacted text
//! and a count of substitutions so callers can prepend an audit header.

use lazy_static::lazy_static;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::path::Path;

lazy_static! {
    /// `https://user:secret@host/repo` → keep host/path, mask user + secret.
    /// Restricted to URL contexts (after `://`) so we do not chew through
    /// arbitrary `user:pass` substrings.
    static ref URL_USERINFO_RE: Regex =
        Regex::new(r"(?P<scheme>[a-zA-Z][a-zA-Z0-9+.-]*://)(?P<user>[^/@\s:]+):(?P<pass>[^@\s]+)@")
            .expect("URL_USERINFO_RE");

    /// `Authorization: Bearer abc123` / `bearer abc123` style.
    /// We match the keyword and replace the trailing token with `****`.
    static ref BEARER_RE: Regex = Regex::new(
        r#"(?i)(?P<lead>(?:authorization\s*[:=]\s*)?(?:bearer|basic|token))[ \t]+(?P<val>[^\s"']+)"#
    )
    .expect("BEARER_RE");

    /// `--token=secret` / `--token secret` / `-p=secret`.
    /// Covers `token`, `password`, `passwd`, `pwd`, `api-key`, `apikey`,
    /// `secret`, `auth`, `access-token`, `refresh-token`.
    static ref FLAG_VALUE_RE: Regex = Regex::new(
        r#"(?ix)
        (?P<flag>--(?:token|password|passwd|pwd|api[_-]?key|secret|auth|access[_-]?token|refresh[_-]?token))
        (?P<sep>=|\s+)
        (?P<val>[^\s"']+)
        "#
    )
    .expect("FLAG_VALUE_RE");

    /// Inline env-style `FOO_TOKEN=value` assignment.
    /// Restricted to a known-allowlist of credential-shaped variable names so
    /// we do not mangle benign `LANG=en_US.UTF-8`-style assignments.
    static ref INLINE_ENV_RE: Regex = Regex::new(
        r"(?x)
        \b
        (?P<name>
            AWS_SECRET_ACCESS_KEY
            | AWS_SESSION_TOKEN
            | AWS_ACCESS_KEY_ID
            | GH_TOKEN
            | GITHUB_TOKEN
            | GH_ENTERPRISE_TOKEN
            | GITLAB_TOKEN
            | OPENAI_API_KEY
            | ANTHROPIC_API_KEY
            | NPM_TOKEN
            | HF_TOKEN
            | HUGGING_FACE_HUB_TOKEN
            | SLACK_TOKEN
            | SLACK_BOT_TOKEN
            | SLACK_APP_TOKEN
            | DOCKER_PASSWORD
            | DOCKERHUB_PASSWORD
            | CI_JOB_TOKEN
            | CARGO_REGISTRY_TOKEN
            | RUBYGEMS_API_KEY
            | PYPI_TOKEN
            | DATABASE_URL
            | PGPASSWORD
        )
        =
        (?P<val>\S+)
        "
    )
    .expect("INLINE_ENV_RE");

    /// "Looks like a credential" heuristic — well-known prefixes.
    /// `ghp_…`, `gho_…`, `ghu_…`, `ghs_…`, `github_pat_…`,
    /// `sk-…` / `sk_live_…` / `sk_test_…`, `xoxb-…` / `xoxp-…`,
    /// `AKIA…` (AWS access key id), `ASIA…` (AWS STS), `glpat-…` (GitLab).
    static ref CREDENTIAL_PREFIX_RE: Regex = Regex::new(
        r"(?x)
        \b
        (?:
            ghp_ | gho_ | ghu_ | ghs_ | github_pat_
            | sk-live_ | sk_live_ | sk_test_ | sk-
            | xoxb- | xoxp- | xoxa- | xoxs-
            | glpat-
            | AKIA | ASIA
        )
        [A-Za-z0-9_\-]{16,}
        \b
        "
    )
    .expect("CREDENTIAL_PREFIX_RE");

    /// Project-path output shape used by `redact_project_path`.
    static ref BASENAME_SAFE_RE: Regex =
        Regex::new(r"[^A-Za-z0-9_.-]").expect("BASENAME_SAFE_RE");
}

/// Scrub credential-shaped substrings from a command line.
///
/// Runs a single, deterministic pass through the lazy-cached regex bundle.
/// The function is idempotent — running it twice produces the same string,
/// because every replacement collapses to `****` (which matches none of the
/// patterns).
pub(crate) fn redact_command(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // 1. URL userinfo: scheme://user:pass@host → scheme://****:****@host
    let s = URL_USERINFO_RE.replace_all(s, "$scheme****:****@");

    // 2. Bearer / Basic / Token header values
    let s = BEARER_RE.replace_all(&s, "$lead ****");

    // 3. Flag/value pairs (--token=…, --password …)
    let s = FLAG_VALUE_RE.replace_all(&s, "$flag$sep****");

    // 4. Well-known inline env assignments (GH_TOKEN=…)
    let s = INLINE_ENV_RE.replace_all(&s, "$name=****");

    // 5. Credential prefix heuristic (ghp_…, sk-…, AKIA…) — last so the
    //    other rules get first crack at structured values.
    let s = CREDENTIAL_PREFIX_RE.replace_all(&s, "****");

    s.into_owned()
}

/// Scrub credential-shaped substrings from free-form command output.
///
/// Like [`redact_command`] but returns the substitution count alongside the
/// redacted string. The count is used by the tee write path to decide whether
/// to prepend the `--- rtk: N credential-like patterns redacted ---` audit
/// header.
///
/// Counted matches are *substituted* occurrences (one per regex hit) — note
/// that a single line can hit multiple patterns (e.g. `Authorization: Bearer
/// ghp_…` matches both the bearer rule and the credential-prefix rule), each
/// adds to the count. This is the desired behaviour: the header just signals
/// "something looked like a credential and was masked", not "exactly N unique
/// secrets were found".
///
/// Reuses the same `lazy_static!`-cached regex bundle as `redact_command`,
/// so the cost is identical to a single command-line redact pass.
pub(crate) fn redact_content(s: &str) -> (String, usize) {
    if s.is_empty() {
        return (String::new(), 0);
    }

    let mut count = 0usize;

    // 1. URL userinfo
    count += URL_USERINFO_RE.find_iter(s).count();
    let s = URL_USERINFO_RE.replace_all(s, "$scheme****:****@");

    // 2. Bearer / Basic / Token header values
    count += BEARER_RE.find_iter(&s).count();
    let s = BEARER_RE.replace_all(&s, "$lead ****");

    // 3. Flag/value pairs
    count += FLAG_VALUE_RE.find_iter(&s).count();
    let s = FLAG_VALUE_RE.replace_all(&s, "$flag$sep****");

    // 4. Inline env assignments
    count += INLINE_ENV_RE.find_iter(&s).count();
    let s = INLINE_ENV_RE.replace_all(&s, "$name=****");

    // 5. Credential prefix heuristic — last so the structured rules above
    //    get first crack at structured values.
    count += CREDENTIAL_PREFIX_RE.find_iter(&s).count();
    let s = CREDENTIAL_PREFIX_RE.replace_all(&s, "****");

    (s.into_owned(), count)
}

/// Reduce a project path to `<basename>#<8-hex-sha256>`.
///
/// Used by the tracking DB so `rtk gain --by-project` can still distinguish
/// projects without storing `/Users/<corporate-username>/Clients/Acme/…`.
/// Empty / unrecognised paths short-circuit to `""` so the existing
/// `project_path != ''` filter in `projects_count()` keeps working.
///
/// The output shape is `<sanitised-basename>#<8-hex>` which always satisfies
/// `^[A-Za-z0-9_.-]+#[0-9a-f]{8}$`.
pub(crate) fn redact_project_path(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Already-redacted paths (basename#deadbeef) are idempotent — let them
    // round-trip unchanged so the one-shot migration is safe to re-run.
    if is_already_hashed(s) {
        return s.to_string();
    }

    let basename = Path::new(s)
        .file_name()
        .map(|os| os.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());

    let basename_safe = BASENAME_SAFE_RE.replace_all(&basename, "_");
    let basename_safe = if basename_safe.is_empty() {
        "root".to_string()
    } else {
        basename_safe.into_owned()
    };

    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{:02x}", byte);
    }

    format!("{}#{}", basename_safe, hex)
}

fn is_already_hashed(s: &str) -> bool {
    let Some((base, hash)) = s.rsplit_once('#') else {
        return false;
    };
    if hash.len() != 8 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    !base.is_empty()
        && base
            .chars()
            .all(|c| matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '.' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_url_userinfo() {
        let input = "git push https://alice:s3cret@github.com/acme/repo.git main";
        let got = redact_command(input);
        assert!(
            got.contains("https://****:****@github.com/acme/repo.git"),
            "got: {got}"
        );
        assert!(!got.contains("alice"));
        assert!(!got.contains("s3cret"));
    }

    #[test]
    fn test_redact_bearer_header() {
        let input = r#"curl -H "Authorization: Bearer abc123XYZdef456" https://api.example.com"#;
        let got = redact_command(input);
        assert!(got.contains("Bearer ****"), "got: {got}");
        assert!(!got.contains("abc123XYZdef456"));
    }

    #[test]
    fn test_redact_flag_value_pairs() {
        let input =
            "tool --token=abc123 --password xyz789 --api-key=key1 --secret xyzS --auth=A1B2C3";
        let got = redact_command(input);
        // Each flag's value masked
        assert!(got.contains("--token=****"), "got: {got}");
        assert!(got.contains("--password ****"), "got: {got}");
        assert!(got.contains("--api-key=****"), "got: {got}");
        assert!(got.contains("--secret ****"), "got: {got}");
        assert!(got.contains("--auth=****"), "got: {got}");
        // No leaked literals
        for needle in ["abc123", "xyz789", "key1", "xyzS", "A1B2C3"] {
            assert!(!got.contains(needle), "needle {needle} leaked: {got}");
        }
    }

    #[test]
    fn test_redact_inline_env() {
        let input = "GH_TOKEN=ghp_ZZZZZZZZZZZZZZZZZZZZ git push origin main";
        let got = redact_command(input);
        assert!(got.contains("GH_TOKEN=****"), "got: {got}");
        assert!(!got.contains("ghp_ZZZZZZZZZZZZZZZZZZZZ"));
    }

    #[test]
    fn test_redact_credential_heuristic() {
        // Synthetic fixtures: chosen so GitHub's secret-scanner does not
        // (correctly) reject the commit. Each starts with a credential-prefix
        // RTK recognises and is followed by 16+ chars that match the regex.
        let inputs_and_haystacks = [
            // ghp_<20 ZZ…> — not a real PAT, but matches our prefix regex.
            "echo ghp_ZZZZZZZZZZZZZZZZZZZZ",
            "echo sk-ZZZZZZZZZZZZZZZZZZZZ",
            "echo AKIAZZZZZZZZZZZZZZZZ",
            "echo xoxb-ZZZZZZZZZZ-ZZZZZZZZZZZZZZ",
        ];
        let prefixes = ["ghp_ZZZZ", "sk-ZZZZ", "AKIAZZZZ", "xoxb-ZZZZ"];
        for input in inputs_and_haystacks {
            let got = redact_command(input);
            assert!(got.contains("****"), "want **** in: {got} (input {input})");
            let prefix_leaked = prefixes
                .iter()
                .any(|p| input.contains(p) && got.contains(p));
            assert!(!prefix_leaked, "prefix leaked: {got}");
        }
    }

    #[test]
    fn test_redact_content_counts_substitutions() {
        let input = "log: GET https://alice:s3cret@host/api Authorization: Bearer abc123XYZdef456";
        let (out, n) = redact_content(input);
        assert!(out.contains("https://****:****@host/api"), "got: {out}");
        assert!(out.contains("Bearer ****"), "got: {out}");
        assert!(!out.contains("alice"));
        assert!(!out.contains("s3cret"));
        assert!(!out.contains("abc123XYZdef456"));
        // URL userinfo + bearer = 2 substitutions at minimum.
        assert!(n >= 2, "expected >=2 matches, got {n}");
    }

    #[test]
    fn test_redact_content_passes_clean_text_unchanged() {
        let input = "running 12 tests\ntest core::tee::tests::test_sanitize_slug ... ok\n";
        let (out, n) = redact_content(input);
        assert_eq!(out, input, "clean text must round-trip");
        assert_eq!(n, 0, "clean text must report zero matches");
    }

    #[test]
    fn test_redact_content_empty_input() {
        let (out, n) = redact_content("");
        assert!(out.is_empty());
        assert_eq!(n, 0);
    }

    #[test]
    fn test_redact_idempotent() {
        let inputs = [
            "git push https://alice:s3cret@github.com/acme/repo.git",
            "curl -H 'Authorization: Bearer abc' https://x",
            "tool --token=xyz --password=k",
            "GH_TOKEN=ghp_ZZZZZZZZZZZZZZZZZZZZ git push",
            "echo ghp_ZZZZZZZZZZZZZZZZZZZZ",
        ];
        for input in inputs {
            let once = redact_command(input);
            let twice = redact_command(&once);
            assert_eq!(once, twice, "non-idempotent for input: {input}");
        }
    }
}

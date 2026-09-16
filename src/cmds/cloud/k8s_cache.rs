//! Detects "no material change" between successive `kubectl get pods` /
//! `get services` calls so an agent polling a stable cluster isn't re-sent
//! the full summary every time.
//!
//! Caches a hash of the already-filtered summary text (not raw JSON) keyed
//! by tool+resource+args, so it stays correct even if the JSON has volatile
//! fields (resourceVersion, timestamps) that never surface in the summary.
//! The underlying `kubectl`/`oc` command still runs on every call — this
//! only skips re-printing an unchanged result, never the real query.

use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::constants::RTK_DATA_DIR;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_FILE: &str = "k8s_cache.json";

/// Env override for the cache file path (same pattern as `RTK_DB_PATH` in
/// `core::tracking`), so tests can point it at a throwaway tempfile instead of
/// the developer's real cache.
const CACHE_PATH_ENV: &str = "RTK_K8S_CACHE_PATH";

/// Sliding poll-gap window: consecutive calls closer together than this keep
/// collapsing to one line. A gap wider than this (agent stepped away, or
/// this is a one-off check) forces a fresh full summary even if unchanged.
const CACHE_TTL_SECS: u64 = 15;

/// Entries not seen for this long are dropped on load: a command nobody has
/// polled in three days is history, not a live cache.
const CACHE_MAX_AGE_SECS: u64 = 3 * 24 * 60 * 60;

/// Hard cap on cache entries; the least-recently-seen are evicted on save.
const CACHE_MAX_ENTRIES: usize = 128;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CacheEntry {
    hash: u64,
    count_line: String,
    last_change_ts: u64,
    last_seen_ts: u64,
}

fn cache_path() -> Option<PathBuf> {
    if let Ok(custom_path) = std::env::var(CACHE_PATH_ENV) {
        return Some(PathBuf::from(custom_path));
    }
    dirs::data_local_dir().map(|d| d.join(RTK_DATA_DIR).join(CACHE_FILE))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load() -> HashMap<String, CacheEntry> {
    let Some(path) = cache_path() else {
        return HashMap::new();
    };
    let mut map: HashMap<String, CacheEntry> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let ts = now();
    map.retain(|_, entry| ts.saturating_sub(entry.last_seen_ts) <= CACHE_MAX_AGE_SECS);
    map
}

fn temp_path_for(path: &std::path::Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp.{}", std::process::id()));
    PathBuf::from(name)
}

fn save(map: &HashMap<String, CacheEntry>) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent()
        && let Err(e) = crate::core::utils::create_private_dir(parent)
    {
        eprintln!(
            "rtk: warning: failed to create k8s cache dir {}: {e}",
            parent.display()
        );
        return;
    }

    // Evict least-recently-seen entries past the cap before serializing.
    let mut capped = map.clone();
    while capped.len() > CACHE_MAX_ENTRIES {
        let Some(oldest) = capped
            .iter()
            .min_by_key(|(_, entry)| entry.last_seen_ts)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        capped.remove(&oldest);
    }

    let json = match serde_json::to_string(&capped) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("rtk: warning: failed to serialize k8s cache: {e}");
            return;
        }
    };

    // Write a sibling temp file, then rename it into place: rename is atomic on
    // one filesystem, so a reader never sees a torn or empty file. The in-process
    // mutex in `check_and_update` serializes threads of *this* process only;
    // separate rtk processes are last-writer-wins, which is fine because the
    // atomic rename means the loser's write is simply replaced whole — never
    // interleaved with the winner's.
    let tmp_path = temp_path_for(&path);
    if let Err(e) = std::fs::write(&tmp_path, &json) {
        eprintln!(
            "rtk: warning: failed to write k8s cache {}: {e}",
            tmp_path.display()
        );
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        eprintln!(
            "rtk: warning: failed to replace k8s cache {}: {e}",
            path.display()
        );
        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// SHA-256 of `s`, truncated to 8 bytes (big-endian `u64`). The hash is
/// persisted to disk and compared across rtk runs, so it must be stable —
/// std's `DefaultHasher` is explicitly unspecified and may change between Rust
/// versions, which would silently invalidate every cached entry.
fn hash_str(s: &str) -> u64 {
    let digest = Sha256::digest(s.as_bytes());
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(buf)
}

/// Build a stable cache key from the resource kind, tool, and passthrough
/// args (namespace/context/label-selector flags land in `args`). The unit
/// separator can't appear inside a single arg, so `["a b", "c"]` no longer
/// collides with `["a", "b c"]` the way a plain space join did.
pub fn cache_key(resource: &str, tool: &str, args: &[String]) -> String {
    format!("{tool}:{resource}:{}", args.join("\u{1f}"))
}

/// Returns `Some(collapsed_message)` if `formatted` is unchanged since the
/// last call for `key` within the poll-gap window, else `None` (caller
/// should print `formatted` as-is). Always records `formatted`'s hash,
/// whether this call is a hit or a miss. `force` always returns `None`
/// (and still refreshes the cache) so `--force` guarantees full detail.
pub fn check_and_update(key: &str, formatted: &str, force: bool) -> Option<String> {
    // Serialize load-modify-save so concurrent threads in this process (e.g.
    // parallel `cargo test` runs, or an agent firing off multiple k8s reads at
    // once) can't clobber each other's cache entries with a stale read. This
    // lock is process-local: separate rtk processes are last-writer-wins, and
    // `save`'s atomic rename is what keeps readers from ever seeing a torn file.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut cache = load();
    let hash = hash_str(formatted);
    let ts = now();
    let count_line = formatted
        .lines()
        .next()
        .unwrap_or("")
        .trim_end()
        .to_string();

    let existing = cache.get(key).cloned();
    let hit = !force
        && existing
            .as_ref()
            .is_some_and(|e| e.hash == hash && ts.saturating_sub(e.last_seen_ts) < CACHE_TTL_SECS);
    // The message is about the gap since the previous query, so it must age off
    // the previous `last_seen_ts` -- `last_change_ts` traces back to the last
    // time the summary actually changed and, under steady polling, can be days
    // old, which would print a misleading "(3 days ago)".
    let previous_seen_ts = existing.as_ref().map(|e| e.last_seen_ts);

    let entry = match (hit, existing) {
        (true, Some(e)) => CacheEntry {
            hash,
            count_line: e.count_line.clone(),
            last_change_ts: e.last_change_ts,
            last_seen_ts: ts,
        },
        _ => CacheEntry {
            hash,
            count_line: count_line.clone(),
            last_change_ts: ts,
            last_seen_ts: ts,
        },
    };

    let message = hit.then(|| {
        let age_secs = previous_seen_ts.map_or(0, |prev| ts.saturating_sub(prev));
        format!(
            "No material change since previous query ({}s ago). {}\nUse --force for fresh detail.\n",
            age_secs, entry.count_line
        )
    });

    cache.insert(key.to_string(), entry);
    save(&cache);
    message
}

/// The value-taking flags on the `kubectl get` path. Only used so a value that
/// literally equals `--force` (e.g. `-n --force`) is not misread as the flag;
/// `-A`/`--all-namespaces` and `--force` themselves stay boolean. Shared with
/// `container::k8s_get_requests_raw_output`, which needs the same value
/// consumption to tell `-o`/`--output` from a flag's value.
pub(crate) fn kubectl_get_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let takes_value = match kind {
        TokenKind::Long => matches!(
            name,
            "namespace"
                | "output"
                | "selector"
                | "field-selector"
                | "context"
                | "kubeconfig"
                | "container"
                | "tail"
                | "sort-by"
        ),
        TokenKind::Short => matches!(name, "n" | "o" | "l" | "c"),
        TokenKind::Positional | TokenKind::DashDash => false,
    };
    takes_value.then(ValueSpec::value)
}

/// Strips rtk's own `--force` flag out of `args`, returning whether it was
/// present and the remaining args (kubectl/oc never see `--force` — it's
/// rtk-only and has no meaning to `get`). Detection goes through the shared
/// tokenizer (repo rule #6, never a raw text scan): `--force=true`/`--force=1`
/// count as force, `--force=false` doesn't but is still consumed, a `--force`
/// after the user's `--` is forwarded untouched, and a `--force` that is
/// really a flag's value (`-n --force`) is left alone.
pub fn extract_force_flag(args: &[String]) -> (bool, Vec<String>) {
    let tokens = arg_tokenizer::tokenize_grammar(args, &kubectl_get_takes_value, Dialect::Posix);
    let mut force = false;
    let mut force_indices = std::collections::HashSet::new();

    for token in arg_tokenizer::before_dashdash(&tokens) {
        if token.kind != TokenKind::Long || !token.double_dash || token.text != "force" {
            continue;
        }
        if !matches!(token.attached, Some("false") | Some("0")) {
            force = true;
        }
        // Strip every spelling, including --force=false: `kubectl get` has no
        // --force flag, so forwarding any of them would fail the command.
        force_indices.insert(token.source_index);
    }

    if force_indices.is_empty() {
        return (false, args.to_vec());
    }

    let rest = args
        .iter()
        .enumerate()
        .filter(|(index, _)| !force_indices.contains(index))
        .map(|(_, arg)| arg.clone())
        .collect();
    (force, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the process-global `RTK_K8S_CACHE_PATH` env
    /// var. Must be one shared static — a static inside each test function is a
    /// distinct lock per function, so the tests would race on the same env var
    /// under parallel `cargo test`. Same rationale as `core::tracking`'s tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Runs `f` with `RTK_K8S_CACHE_PATH` pointed at a fresh tempdir's cache
    /// file, so tests never read or write the developer's real cache.
    fn with_temp_cache(f: impl FnOnce(&std::path::Path)) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(CACHE_FILE);
        temp_env::with_var(CACHE_PATH_ENV, Some(&path), || f(&path));
    }

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn first_call_is_always_a_miss() {
        with_temp_cache(|_| {
            assert!(check_and_update("pods:kubectl:-A", "3 pods: 3\n", false).is_none());
        });
    }

    #[test]
    fn identical_second_call_collapses() {
        with_temp_cache(|_| {
            let key = "pods:kubectl:-A";
            let summary = "3 pods: 3\n";
            assert!(check_and_update(key, summary, false).is_none());
            let hit = check_and_update(key, summary, false).expect("second call should hit");
            assert!(hit.contains("No material change"));
        });
    }

    #[test]
    fn changed_state_is_a_miss() {
        with_temp_cache(|_| {
            let key = "pods:kubectl:-A";
            assert!(check_and_update(key, "3 pods: 3\n", false).is_none());
            // one pod started crashing -- summary text differs
            assert!(check_and_update(key, "3 pods: 2, 1 [x]\n", false).is_none());
        });
    }

    #[test]
    fn force_always_bypasses_cache() {
        with_temp_cache(|_| {
            let key = "pods:kubectl:-A";
            let summary = "3 pods: 3\n";
            assert!(check_and_update(key, summary, false).is_none());
            assert!(check_and_update(key, summary, true).is_none());
        });
    }

    #[test]
    fn expired_entry_is_a_miss_and_emits_full_text() {
        with_temp_cache(|_| {
            let key = "pods:kubectl:-A";
            let summary = "3 pods: 3\n";
            // Seed an entry seen longer ago than the poll-gap window, as if the
            // agent stepped away between queries.
            let stale_ts = now().saturating_sub(CACHE_TTL_SECS + 60);
            let mut map = HashMap::new();
            map.insert(
                key.to_string(),
                CacheEntry {
                    hash: hash_str(summary),
                    count_line: summary.lines().next().unwrap_or("").to_string(),
                    last_change_ts: stale_ts,
                    last_seen_ts: stale_ts,
                },
            );
            save(&map);

            // None means the caller prints the full summary.
            assert!(check_and_update(key, summary, false).is_none());
        });
    }

    #[test]
    fn corrupt_cache_file_falls_back_to_full_output_without_panicking() {
        with_temp_cache(|path| {
            std::fs::write(path, "{ this is not json").expect("write garbage");
            assert!(check_and_update("pods:kubectl:-A", "3 pods: 3\n", false).is_none());
        });
    }

    #[test]
    fn collapse_age_uses_previous_seen_ts_not_last_change_ts() {
        with_temp_cache(|_| {
            let key = "pods:kubectl:-A";
            let summary = "3 pods: 3\n";
            let ts = now();
            let mut map = HashMap::new();
            map.insert(
                key.to_string(),
                CacheEntry {
                    hash: hash_str(summary),
                    count_line: summary.lines().next().unwrap_or("").to_string(),
                    // Changed long ago but polled 5s ago: the message must
                    // describe the poll gap, not the stale change age.
                    last_change_ts: ts.saturating_sub(500),
                    last_seen_ts: ts.saturating_sub(5),
                },
            );
            save(&map);

            let message = check_and_update(key, summary, false).expect("should hit");
            assert!(message.contains("(5s ago)"), "got: {message}");
            assert!(!message.contains("500"), "got: {message}");
        });
    }

    #[test]
    fn distinct_keys_do_not_clobber_each_other() {
        with_temp_cache(|_| {
            let pods = "pods:kubectl:-A";
            let services = "services:kubectl:-A";
            assert!(check_and_update(pods, "3 pods: 3\n", false).is_none());
            assert!(check_and_update(services, "2 services:\n", false).is_none());
            assert!(check_and_update(pods, "3 pods: 3\n", false).is_some());
            assert!(check_and_update(services, "2 services:\n", false).is_some());
        });
    }

    #[test]
    fn cache_key_distinguishes_argument_boundaries() {
        let first = cache_key("pods", "kubectl", &["a b".to_string(), "c".to_string()]);
        let second = cache_key("pods", "kubectl", &["a".to_string(), "b c".to_string()]);
        assert_ne!(first, second);
    }

    #[test]
    fn extract_force_flag_strips_bare_and_attached_force() {
        let args = owned(&["-A", "--force", "-n", "default"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(force);
        assert_eq!(rest, vec!["-A", "-n", "default"]);

        let args = owned(&["--force=true", "-A"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(force);
        assert_eq!(rest, vec!["-A"]);

        let args = owned(&["--force=1"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(force);
        assert!(rest.is_empty());
    }

    #[test]
    fn explicit_force_false_is_not_force_but_is_still_stripped() {
        let args = owned(&["--force=false", "-A"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(!force);
        assert_eq!(rest, vec!["-A"]);
    }

    #[test]
    fn force_after_double_dash_is_forwarded_untouched() {
        let args = owned(&["-A", "--", "--force"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(!force);
        assert_eq!(rest, vec!["-A", "--", "--force"]);
    }

    #[test]
    fn force_as_a_flag_value_is_not_detected() {
        let args = owned(&["-n", "--force"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(!force);
        assert_eq!(rest, vec!["-n", "--force"]);
    }

    #[test]
    fn extract_force_flag_absent() {
        let args = owned(&["-A"]);
        let (force, rest) = extract_force_flag(&args);
        assert!(!force);
        assert_eq!(rest, vec!["-A"]);
    }
}

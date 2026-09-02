//! Detects "no material change" between successive `kubectl get pods` /
//! `get services` calls so an agent polling a stable cluster isn't re-sent
//! the full summary every time.
//!
//! Caches a hash of the already-filtered summary text (not raw JSON) keyed
//! by tool+resource+args, so it stays correct even if the JSON has volatile
//! fields (resourceVersion, timestamps) that never surface in the summary.
//! The underlying `kubectl`/`oc` command still runs on every call — this
//! only skips re-printing an unchanged result, never the real query.

use super::constants::RTK_DATA_DIR;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_FILE: &str = "k8s_cache.json";

/// Sliding poll-gap window: consecutive calls closer together than this keep
/// collapsing to one line. A gap wider than this (agent stepped away, or
/// this is a one-off check) forces a fresh full summary even if unchanged.
const CACHE_TTL_SECS: u64 = 15;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CacheEntry {
    hash: u64,
    count_line: String,
    last_change_ts: u64,
    last_seen_ts: u64,
}

fn cache_path() -> Option<PathBuf> {
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
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(map: &HashMap<String, CacheEntry>) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(map) {
        let _ = std::fs::write(path, json);
    }
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Build a stable cache key from the resource kind, tool, and passthrough
/// args (namespace/context/label-selector flags land in `args`).
pub fn cache_key(resource: &str, tool: &str, args: &[String]) -> String {
    format!("{tool}:{resource}:{}", args.join(" "))
}

/// Returns `Some(collapsed_message)` if `formatted` is unchanged since the
/// last call for `key` within the poll-gap window, else `None` (caller
/// should print `formatted` as-is). Always records `formatted`'s hash,
/// whether this call is a hit or a miss. `force` always returns `None`
/// (and still refreshes the cache) so `--force` guarantees full detail.
pub fn check_and_update(key: &str, formatted: &str, force: bool) -> Option<String> {
    // Serialize load-modify-save so concurrent in-process calls (e.g. parallel
    // `cargo test` threads, or an agent firing off multiple k8s reads at once)
    // can't clobber each other's cache entries with a stale read.
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

    let entry = if hit {
        let e = existing.expect("hit implies existing entry");
        CacheEntry {
            hash,
            count_line: e.count_line.clone(),
            last_change_ts: e.last_change_ts,
            last_seen_ts: ts,
        }
    } else {
        CacheEntry {
            hash,
            count_line: count_line.clone(),
            last_change_ts: ts,
            last_seen_ts: ts,
        }
    };

    let message = hit.then(|| {
        format!(
            "No material change since previous query ({}s ago). {}\nUse --force for fresh detail.\n",
            ts.saturating_sub(entry.last_change_ts),
            entry.count_line
        )
    });

    cache.insert(key.to_string(), entry);
    save(&cache);
    message
}

/// Strips a bare `--force` flag out of `args`, returning whether it was
/// present and the remaining args (kubectl/oc never see `--force` — it's
/// rtk-only and has no meaning to `get`).
pub fn extract_force_flag(args: &[String]) -> (bool, Vec<String>) {
    let mut force = false;
    let rest = args
        .iter()
        .filter(|a| {
            if a.as_str() == "--force" {
                force = true;
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();
    (force, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_key(name: &str) -> String {
        // Isolate each test's cache entry so parallel `cargo test` runs
        // (sharing the same on-disk cache file) can't clobber each other.
        format!("test:{name}:{}", std::process::id())
    }

    #[test]
    fn first_call_is_always_a_miss() {
        let key = unique_key("first_call");
        assert!(check_and_update(&key, "3 pods: 3\n", false).is_none());
    }

    #[test]
    fn identical_second_call_collapses() {
        let key = unique_key("identical");
        let summary = "3 pods: 3\n";
        assert!(check_and_update(&key, summary, false).is_none());
        let hit = check_and_update(&key, summary, false);
        assert!(hit.is_some());
        assert!(hit.unwrap().contains("No material change"));
    }

    #[test]
    fn changed_state_is_a_miss() {
        let key = unique_key("changed");
        assert!(check_and_update(&key, "3 pods: 3\n", false).is_none());
        // one pod started crashing -- summary text differs
        assert!(check_and_update(&key, "3 pods: 2, 1 [x]\n", false).is_none());
    }

    #[test]
    fn force_always_bypasses_cache() {
        let key = unique_key("force");
        let summary = "3 pods: 3\n";
        assert!(check_and_update(&key, summary, false).is_none());
        assert!(check_and_update(&key, summary, true).is_none());
    }

    #[test]
    fn extract_force_flag_strips_only_force() {
        let args = vec![
            "-A".to_string(),
            "--force".to_string(),
            "-n".to_string(),
            "default".to_string(),
        ];
        let (force, rest) = extract_force_flag(&args);
        assert!(force);
        assert_eq!(rest, vec!["-A", "-n", "default"]);
    }

    #[test]
    fn extract_force_flag_absent() {
        let args = vec!["-A".to_string()];
        let (force, rest) = extract_force_flag(&args);
        assert!(!force);
        assert_eq!(rest, vec!["-A".to_string()]);
    }
}

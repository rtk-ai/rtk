//! Per-layer aggressivity and the fallback exclude list, resolved once from
//! env/config.

use super::decorative::DecorativeLevel;
use super::dedup::DedupLevel;

/// How aggressively to cap "show N items, +M more" lists. `Reasonable` = today's
/// values; `High` caps tighter (more compression), `Light` looser, `None` = no cap.
/// A dial only — the scaling lives in `core::truncate::caps()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TruncateLevel {
    None,
    Light,
    #[default]
    Reasonable,
    High,
}

impl TruncateLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(Self::None),
            "light" | "low" => Some(Self::Light),
            "reasonable" | "normal" | "default" | "medium" | "med" => Some(Self::Reasonable),
            "high" | "aggressive" => Some(Self::High),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Levels {
    pub decorative: DecorativeLevel,
    pub dedup: DedupLevel,
    pub truncate: TruncateLevel,
}

// Raw-output commands: their content must stay byte-exact, so the global
// fallback pipeline never touches them. Users extend this via [layers].exclude.
const BUILTIN_EXCLUDE: &[&str] = &[
    "cat", "head", "tail", "base64", "xxd", "hexdump", "od", "strings", "dd",
];

// Maps each cmds/ folder group to its commands (the rtk subcommand names). This
// is the per-group config surface: `[layers.<group>]` / RTK_<GROUP>_<LAYER>_LEVEL.
// The runtime can't see a command's source folder, so the mapping lives here.
// Adding a command: add it to its folder's list (see src/cmds/README.md). An
// unlisted command simply falls through to the global `[layers]` (fails open).
// `groups_match_subcommands` (main.rs) guards against typos/renames.
pub const GROUPS: &[(&str, &[&str])] = &[
    ("git", &["git", "gh", "glab", "gt", "diff"]),
    ("rust", &["cargo", "err", "test"]),
    (
        "js",
        &[
            "pnpm",
            "npm",
            "npx",
            "jest",
            "vitest",
            "prisma",
            "tsc",
            "next",
            "lint",
            "prettier",
            "playwright",
        ],
    ),
    ("python", &["ruff", "pytest", "mypy", "pip"]),
    ("go", &["go", "golangci-lint"]),
    ("dotnet", &["dotnet"]),
    ("jvm", &["gradlew"]),
    (
        "cloud",
        &["aws", "psql", "docker", "kubectl", "curl", "wget"],
    ),
    ("ruby", &["rake", "rubocop", "rspec"]),
    (
        "system",
        &[
            "ls", "tree", "read", "smart", "json", "deps", "env", "find", "log", "summary", "grep",
            "wc", "format", "pipe",
        ],
    ),
];

/// The cmds/ folder group a command belongs to, or None (→ global `[layers]`).
pub fn group_for_command(cmd: &str) -> Option<&'static str> {
    GROUPS
        .iter()
        .find(|(_, cmds)| cmds.contains(&cmd))
        .map(|(g, _)| *g)
}

use crate::core::config::{GroupLayers, LayersConfig};
use std::sync::OnceLock;

// The running command's cmds/ folder group (git, js, …). One command per process,
// so a write-once global is enough. Set at dispatch, before any level read.
static CURRENT_GROUP: OnceLock<String> = OnceLock::new();

pub fn set_group(group: &str) {
    let _ = CURRENT_GROUP.set(group.to_string());
}

fn current_group() -> Option<&'static str> {
    CURRENT_GROUP.get().map(String::as_str)
}

struct Resolved {
    levels: Levels,
    exclude: Vec<String>,
}

fn resolved() -> &'static Resolved {
    static RESOLVED: OnceLock<Resolved> = OnceLock::new();
    RESOLVED.get_or_init(resolve)
}

// Precedence, highest first: group env (`RTK_<GROUP>_<LAYER>_LEVEL`) >
// group config (`[layers.<group>]`) > global env (`RTK_<LAYER>_LEVEL`) >
// global config (`[layers]`) > default.
fn resolve_level<T>(
    group: Option<&str>,
    layer: &str,
    parse: fn(&str) -> Option<T>,
    layers: Option<&LayersConfig>,
    group_val: impl Fn(&GroupLayers) -> Option<&str>,
    global_val: impl Fn(&LayersConfig) -> &str,
) -> Option<T> {
    if let Some(g) = group {
        // Env var names can't carry hyphens (e.g. golangci-lint), so normalize.
        let g_env = g.to_uppercase().replace('-', "_");
        if let Ok(v) = std::env::var(format!("RTK_{}_{}_LEVEL", g_env, layer)) {
            if let Some(p) = parse(&v) {
                return Some(p);
            }
        }
        if let Some(gl) = layers.and_then(|l| l.groups.get(g)) {
            if let Some(p) = group_val(gl).and_then(parse) {
                return Some(p);
            }
        }
    }
    if let Ok(v) = std::env::var(format!("RTK_{}_LEVEL", layer)) {
        if let Some(p) = parse(&v) {
            return Some(p);
        }
    }
    layers.and_then(|l| parse(global_val(l)))
}

fn resolve() -> Resolved {
    let config = crate::core::config::Config::load().ok();
    let layers = config.as_ref().map(|c| &c.layers);
    let group = current_group();

    let decorative = resolve_level(
        group,
        "DECORATIVE",
        DecorativeLevel::parse,
        layers,
        |g| g.decorative.as_deref(),
        |c| c.decorative.as_str(),
    )
    .unwrap_or_default();

    let dedup = resolve_level(
        group,
        "DEDUP",
        DedupLevel::parse,
        layers,
        |g| g.dedup.as_deref(),
        |c| c.dedup.as_str(),
    )
    .unwrap_or_default();

    let truncate = resolve_level(
        group,
        "TRUNCATE",
        TruncateLevel::parse,
        layers,
        |g| g.truncate.as_deref(),
        |c| c.truncate.as_str(),
    )
    .unwrap_or_default();

    let mut exclude: Vec<String> = BUILTIN_EXCLUDE.iter().map(|s| s.to_string()).collect();
    if let Some(l) = layers {
        exclude.extend(l.exclude.iter().cloned());
    }

    Resolved {
        levels: Levels {
            decorative,
            dedup,
            truncate,
        },
        exclude,
    }
}

/// Resolved levels, cached to keep config off the hot path (<10ms startup).
pub fn current() -> &'static Levels {
    &resolved().levels
}

pub fn is_excluded(command: &str) -> bool {
    resolved().exclude.iter().any(|c| c == command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(global: &str, group: &str, group_val: Option<&str>) -> LayersConfig {
        let mut groups = std::collections::HashMap::new();
        if let Some(v) = group_val {
            groups.insert(
                group.to_string(),
                GroupLayers {
                    decorative: Some(v.to_string()),
                    dedup: None,
                    truncate: None,
                },
            );
        }
        LayersConfig {
            decorative: global.to_string(),
            dedup: "none".to_string(),
            truncate: "reasonable".to_string(),
            exclude: Vec::new(),
            groups,
        }
    }

    fn resolve_dec(group: Option<&str>, c: &LayersConfig) -> Option<DecorativeLevel> {
        resolve_level(
            group,
            "DECORATIVE",
            DecorativeLevel::parse,
            Some(c),
            |g| g.decorative.as_deref(),
            |c| c.decorative.as_str(),
        )
    }

    #[test]
    fn group_config_overrides_global() {
        let c = cfg("light", "js", Some("high"));
        assert_eq!(resolve_dec(Some("js"), &c), Some(DecorativeLevel::High));
    }

    #[test]
    fn global_used_when_group_has_no_override() {
        let c = cfg("light", "js", None);
        assert_eq!(resolve_dec(Some("js"), &c), Some(DecorativeLevel::Light));
        assert_eq!(resolve_dec(Some("git"), &c), Some(DecorativeLevel::Light));
    }

    #[test]
    fn no_group_uses_global() {
        let c = cfg("high", "js", Some("none"));
        assert_eq!(resolve_dec(None, &c), Some(DecorativeLevel::High));
    }

    #[test]
    fn group_env_name_normalizes_hyphens() {
        // group "golangci-lint" must read RTK_GOLANGCI_LINT_*, not RTK_GOLANGCI-LINT_*.
        std::env::set_var("RTK_GOLANGCI_LINT_DECORATIVE_LEVEL", "high");
        let c = cfg("light", "golangci-lint", None);
        let got = resolve_dec(Some("golangci-lint"), &c);
        std::env::remove_var("RTK_GOLANGCI_LINT_DECORATIVE_LEVEL");
        assert_eq!(got, Some(DecorativeLevel::High));
    }
}

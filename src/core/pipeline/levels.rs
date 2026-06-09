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
// fallback pipeline never touches them. Users extend this via [levels].exclude.
const BUILTIN_EXCLUDE: &[&str] = &[
    "cat", "head", "tail", "base64", "xxd", "hexdump", "od", "strings", "dd",
];

struct Resolved {
    levels: Levels,
    exclude: Vec<String>,
}

fn resolved() -> &'static Resolved {
    use std::sync::OnceLock;
    static RESOLVED: OnceLock<Resolved> = OnceLock::new();
    RESOLVED.get_or_init(resolve)
}

fn resolve() -> Resolved {
    let config = crate::core::config::Config::load().ok();
    let decorative = std::env::var("RTK_DECORATIVE_LEVEL")
        .ok()
        .and_then(|v| DecorativeLevel::parse(&v))
        .or_else(|| {
            config
                .as_ref()
                .and_then(|c| DecorativeLevel::parse(&c.levels.decorative))
        })
        .unwrap_or_default();

    let dedup = std::env::var("RTK_DEDUP_LEVEL")
        .ok()
        .and_then(|v| DedupLevel::parse(&v))
        .or_else(|| {
            config
                .as_ref()
                .and_then(|c| DedupLevel::parse(&c.levels.dedup))
        })
        .unwrap_or_default();

    let truncate = std::env::var("RTK_TRUNCATE_LEVEL")
        .ok()
        .and_then(|v| TruncateLevel::parse(&v))
        .or_else(|| {
            config
                .as_ref()
                .and_then(|c| TruncateLevel::parse(&c.levels.truncate))
        })
        .unwrap_or_default();

    let mut exclude: Vec<String> = BUILTIN_EXCLUDE.iter().map(|s| s.to_string()).collect();
    if let Some(c) = &config {
        exclude.extend(c.levels.exclude.iter().cloned());
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

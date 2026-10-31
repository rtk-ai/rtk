//! Shared compression-level scale used by the level-based transforms
//! (ingest filter, image optimization) and the admin Settings API.

use std::sync::{Arc, RwLock};

/// Transform aggressiveness, selectable at runtime from the admin Settings.
/// Each transform maps the level to its own thresholds; `Off` disables it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionLevel {
    #[default]
    Off,
    Light,
    Balanced,
    Aggressive,
}

impl CompressionLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompressionLevel::Off => "off",
            CompressionLevel::Light => "light",
            CompressionLevel::Balanced => "balanced",
            CompressionLevel::Aggressive => "aggressive",
        }
    }

    /// Parse a level name (case-insensitive); `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" | "none" | "0" => Some(CompressionLevel::Off),
            "light" => Some(CompressionLevel::Light),
            "balanced" | "medium" => Some(CompressionLevel::Balanced),
            "aggressive" | "high" | "max" => Some(CompressionLevel::Aggressive),
            _ => None,
        }
    }

    /// All levels, for populating the Settings selector.
    pub fn all() -> [CompressionLevel; 4] {
        [
            CompressionLevel::Off,
            CompressionLevel::Light,
            CompressionLevel::Balanced,
            CompressionLevel::Aggressive,
        ]
    }
}

/// Shared, runtime-mutable compression level (set from the admin Settings).
pub type SharedCompression = Arc<RwLock<CompressionLevel>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_and_names() {
        assert_eq!(
            CompressionLevel::parse("BALANCED"),
            Some(CompressionLevel::Balanced)
        );
        assert_eq!(
            CompressionLevel::parse("high"),
            Some(CompressionLevel::Aggressive)
        );
        assert_eq!(CompressionLevel::parse("nope"), None);
        assert_eq!(CompressionLevel::Light.as_str(), "light");
        assert_eq!(CompressionLevel::default(), CompressionLevel::Off);
    }
}

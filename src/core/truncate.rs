//! Global truncation caps shared by every filter. See `src/core/README.md`
//! ("Truncation Caps") for the cap classes, config policy, and deviation rules.

use crate::core::pipeline::TruncateLevel;

/// Errors: most actionable, shown the most.
pub const CAP_ERRORS: usize = 20;
/// Warnings and test failures: lower signal density than errors.
pub const CAP_WARNINGS: usize = 10;
/// Flat lists (PRs, services, packages): one line per item.
pub const CAP_LIST: usize = 20;
/// Inventories (`pip list`, `docker images`): exhaustive lookups.
pub const CAP_INVENTORY: usize = 50;

/// A cap reduced for a verbose data class. Falls back to `cap` when `by >= cap`
/// so a deviation can never empty the list; `0` stays `0`. `const fn`, underflow-safe.
pub const fn reduced(cap: usize, by: usize) -> usize {
    if by < cap {
        cap - by
    } else {
        cap
    }
}

/// The four item caps, scaled by the resolved `truncate` level.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub errors: usize,
    pub warnings: usize,
    pub list: usize,
    pub inventory: usize,
}

/// Item caps for this invocation. `Reasonable` (default) returns the `CAP_*`
/// values above unchanged; `High` halves (more compression), `Light` doubles,
/// `None` removes the cap.
pub fn caps() -> Caps {
    let level = crate::core::pipeline::truncate_level();
    Caps {
        errors: scale(CAP_ERRORS, level),
        warnings: scale(CAP_WARNINGS, level),
        list: scale(CAP_LIST, level),
        inventory: scale(CAP_INVENTORY, level),
    }
}

fn scale(base: usize, level: TruncateLevel) -> usize {
    match level {
        TruncateLevel::None => usize::MAX,
        TruncateLevel::Light => base.saturating_mul(2),
        TruncateLevel::Reasonable => base,
        TruncateLevel::High => (base / 2).max(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_preserves_current_values() {
        assert_eq!(reduced(CAP_WARNINGS, 5), 5);
        assert_eq!(reduced(CAP_LIST, 5), 15);
    }

    #[test]
    fn reduced_falls_back_to_cap_when_offset_too_large() {
        assert_eq!(reduced(4, 5), 4);
        assert_eq!(reduced(5, 5), 5);
    }

    #[test]
    fn reduced_honors_zero_cap() {
        assert_eq!(reduced(0, 5), 0);
    }

    #[test]
    fn scale_reasonable_is_identity() {
        assert_eq!(scale(CAP_ERRORS, TruncateLevel::Reasonable), CAP_ERRORS);
        assert_eq!(scale(CAP_LIST, TruncateLevel::Reasonable), CAP_LIST);
    }

    #[test]
    fn scale_high_halves_light_doubles_none_unlimited() {
        assert_eq!(scale(20, TruncateLevel::High), 10);
        assert_eq!(scale(20, TruncateLevel::Light), 40);
        assert_eq!(scale(20, TruncateLevel::None), usize::MAX);
        assert_eq!(scale(1, TruncateLevel::High), 1); // never empties
    }

    #[test]
    fn truncate_level_parses() {
        assert_eq!(TruncateLevel::parse("none"), Some(TruncateLevel::None));
        assert_eq!(TruncateLevel::parse("HIGH"), Some(TruncateLevel::High));
        assert_eq!(TruncateLevel::parse("bogus"), None);
    }

    // Sweep every plausible (cap, by) a future config could produce and assert
    // the invariants that make caps safe: the result never wraps past `cap`, and
    // the offset never empties a non-zero cap. `usize::MAX` covers a wraparound bug.
    #[test]
    fn reduced_is_underflow_safe_across_all_inputs() {
        for cap in 0..=64usize {
            for by in [0usize, 1, 5, 10, 64, usize::MAX] {
                let r = reduced(cap, by);
                assert!(r <= cap, "reduced({cap}, {by}) = {r} exceeds cap (wrapped)");
                if cap == 0 {
                    assert_eq!(r, 0, "zero cap must stay zero");
                } else {
                    assert!(r >= 1, "reduced({cap}, {by}) = {r} emptied a non-zero cap");
                }
                if by < cap {
                    assert_eq!(r, cap - by, "exact deviation must be preserved");
                }
            }
        }
    }
}

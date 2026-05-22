//! Global truncation caps shared by every filter. See `src/core/README.md`
//! ("Truncation Caps") for the cap classes, config policy, and deviation rules.

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

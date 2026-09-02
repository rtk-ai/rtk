use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static INITIALIZED: OnceLock<()> = OnceLock::new();
static TELEMETRY_FORCE_DISABLED: AtomicBool = AtomicBool::new(false);
static TRACKING_FORCE_DISABLED: AtomicBool = AtomicBool::new(false);

fn init() {
    INITIALIZED.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--secure") {
            TELEMETRY_FORCE_DISABLED.store(true, Ordering::Relaxed);
            TRACKING_FORCE_DISABLED.store(true, Ordering::Relaxed);
        } else {
            if args.iter().any(|a| a == "--no-telemetry") {
                TELEMETRY_FORCE_DISABLED.store(true, Ordering::Relaxed);
            }
            if args.iter().any(|a| a == "--no-tracking") {
                TRACKING_FORCE_DISABLED.store(true, Ordering::Relaxed);
            }
        }
    });
}

pub fn set_secure() {
    init();
    TELEMETRY_FORCE_DISABLED.store(true, Ordering::Relaxed);
    TRACKING_FORCE_DISABLED.store(true, Ordering::Relaxed);
}

pub fn set_no_telemetry() {
    init();
    TELEMETRY_FORCE_DISABLED.store(true, Ordering::Relaxed);
}

pub fn set_no_tracking() {
    init();
    TRACKING_FORCE_DISABLED.store(true, Ordering::Relaxed);
}

pub fn is_telemetry_forced_off() -> bool {
    init();
    TELEMETRY_FORCE_DISABLED.load(Ordering::Relaxed)
}

pub fn is_tracking_forced_off() -> bool {
    init();
    TRACKING_FORCE_DISABLED.load(Ordering::Relaxed)
}

/// Reset all secure state — only available in tests.
#[cfg(test)]
pub fn reset_for_tests() {
    TELEMETRY_FORCE_DISABLED.store(false, Ordering::Relaxed);
    TRACKING_FORCE_DISABLED.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        TELEMETRY_FORCE_DISABLED.store(false, Ordering::Relaxed);
        TRACKING_FORCE_DISABLED.store(false, Ordering::Relaxed);
    }

    #[test]
    fn test_initial_state_is_false() {
        reset();
        assert!(!is_telemetry_forced_off());
        assert!(!is_tracking_forced_off());
    }

    #[test]
    fn test_secure_disables_both() {
        reset();
        set_secure();
        assert!(is_telemetry_forced_off());
        assert!(is_tracking_forced_off());
    }

    #[test]
    fn test_no_telemetry_disables_telemetry_only() {
        reset();
        set_no_telemetry();
        assert!(is_telemetry_forced_off());
        assert!(!is_tracking_forced_off());
    }

    #[test]
    fn test_no_tracking_disables_tracking_only() {
        reset();
        set_no_tracking();
        assert!(!is_telemetry_forced_off());
        assert!(is_tracking_forced_off());
    }

    #[test]
    fn test_secure_overrides_individual_flags() {
        reset();
        set_no_telemetry();
        set_secure();
        assert!(is_telemetry_forced_off());
        assert!(is_tracking_forced_off());
    }

    #[test]
    fn test_setters_are_idempotent() {
        reset();
        set_secure();
        set_secure();
        assert!(is_telemetry_forced_off());
        assert!(is_tracking_forced_off());
    }
}

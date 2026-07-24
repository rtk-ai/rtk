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

//! Shared test utilities for the cmd module.

use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// RAII guard that serializes env-var-mutating tests and auto-cleans on drop.
/// Prevents race conditions between parallel test threads and ensures cleanup
/// even if a test panics.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    pub fn new() -> Self {
        let lock = ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Self::cleanup();
        Self { _lock: lock }
    }

    fn cleanup() {
        std::env::remove_var("RTK_SAFE_COMMANDS");
        std::env::remove_var("RTK_BLOCK_TOKEN_WASTE");
        std::env::remove_var("RTK_ACTIVE");
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        Self::cleanup();
    }
}

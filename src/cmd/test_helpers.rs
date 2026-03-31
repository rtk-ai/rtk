use std::sync::{Mutex, MutexGuard, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
        std::env::remove_var("RTK_DB_PATH");
        std::env::remove_var("RTK_HOOK_ENABLED");
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        Self::cleanup();
    }
}

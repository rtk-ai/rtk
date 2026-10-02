//! The environment variables rtk reads: rtk's own switches (`RTK_TEE=0`,
//! `RTK_NO_TOML=1`, …), its path overrides (`RTK_DB_PATH`, …), and the agent,
//! CI and tool variables it consults (`CLAUDE_CONFIG_DIR`, `CI`,
//! `COMPOSER_BIN_DIR`, …). Paths are read through `user_dirs::env_path`, which
//! resolves here.
//!
//! A test build does not read them from the process environment, which is the
//! developer's shell: an exported `RTK_NO_TOML=1` would switch the TOML engine
//! off under every test that expects it on, and an exported `CLAUDE_CONFIG_DIR`
//! would point a test at their real settings. A test sets the ones it needs
//! with [`with_vars`] or [`with_path`], which hold them for the calling thread
//! only, so tests running in parallel never see each other's.
//!
//! Reading one anywhere else bypasses that, so
//! `test_isolation::only_user_dirs_resolves_user_locations` refuses it.

#[cfg(test)]
use std::cell::RefCell;
#[cfg(test)]
use std::collections::HashMap;
use std::ffi::OsString;

/// The variable's value, `None` when it is unset or not valid UTF-8.
///
/// In a test build, only what the calling thread set.
pub fn var(name: &str) -> Option<String> {
    #[cfg(not(test))]
    {
        std::env::var(name).ok()
    }
    #[cfg(test)]
    {
        var_os(name)?.into_string().ok()
    }
}

/// The variable's value as the platform holds it, `None` when it is unset.
///
/// In a test build, only what the calling thread set.
pub fn var_os(name: &str) -> Option<OsString> {
    #[cfg(not(test))]
    {
        std::env::var_os(name)
    }
    #[cfg(test)]
    {
        SET.with(|set| set.borrow().get(name).cloned())
    }
}

#[cfg(test)]
thread_local! {
    static SET: RefCell<HashMap<String, OsString>> = RefCell::new(HashMap::new());
}

/// Run `f` with each variable set, or unset for `None`, as [`var`] and
/// [`var_os`] see it on this thread. The previous values come back when `f`
/// returns or panics.
#[cfg(test)]
pub fn with_vars<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
    set_while(
        vars.iter()
            .map(|&(name, value)| (name, value.map(OsString::from)))
            .collect(),
        f,
    )
}

/// [`with_vars`] for one variable holding a path.
#[cfg(test)]
pub fn with_path<P: AsRef<std::ffi::OsStr>, R>(
    name: &str,
    value: Option<P>,
    f: impl FnOnce() -> R,
) -> R {
    set_while(vec![(name, value.map(|v| v.as_ref().to_owned()))], f)
}

#[cfg(test)]
fn set_while<R>(vars: Vec<(&str, Option<OsString>)>, f: impl FnOnce() -> R) -> R {
    struct Restore(Vec<(String, Option<OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SET.with(|set| {
                let mut set = set.borrow_mut();
                for (name, previous) in self.0.drain(..).rev() {
                    match previous {
                        Some(value) => set.insert(name, value),
                        None => set.remove(&name),
                    };
                }
            });
        }
    }

    let _restore = SET.with(|set| {
        let mut set = set.borrow_mut();
        Restore(
            vars.into_iter()
                .map(|(name, value)| {
                    let previous = match value {
                        Some(value) => set.insert(name.to_string(), value),
                        None => set.remove(name),
                    };
                    (name.to_string(), previous)
                })
                .collect(),
        )
    });
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_environment_is_not_read() {
        // Any variable the process has: a test build that fell back to the
        // process environment would see it.
        let Some(name) = std::env::vars_os().find_map(|(name, _)| name.into_string().ok()) else {
            return;
        };
        assert_eq!(var_os(&name), None, "{name} leaked through");
    }

    #[test]
    fn values_nest_and_come_back() {
        with_vars(&[("RTK_X", Some("1"))], || {
            assert_eq!(var("RTK_X").as_deref(), Some("1"));
            with_vars(&[("RTK_X", None), ("RTK_Y", Some("2"))], || {
                assert_eq!(var("RTK_X"), None);
                assert_eq!(var("RTK_Y").as_deref(), Some("2"));
            });
            assert_eq!(var("RTK_X").as_deref(), Some("1"));
            assert_eq!(var("RTK_Y"), None);
        });
        assert_eq!(var("RTK_X"), None);
    }

    #[test]
    fn another_thread_does_not_see_them() {
        with_path("RTK_X", Some("/somewhere"), || {
            assert!(var_os("RTK_X").is_some());
            let seen = std::thread::spawn(|| var_os("RTK_X"))
                .join()
                .expect("thread");
            assert_eq!(seen, None);
        });
    }
}

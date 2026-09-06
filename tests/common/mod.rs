//! Shared setup for the integration tests.

/// Point the recall store at a throwaway DB for every `rtk` subprocess this
/// test binary spawns.
///
/// Integration tests run the real binary, which is built without `cfg(test)`
/// and so reads the developer's ambient config — writing their fixture output
/// into `~/.local/share/rtk/recall.db` and leaking fixture slugs into the
/// daily telemetry ping. The unit-test guard in `core::tee` cannot
/// cover this: it is compiled out of the very binary under test.
///
/// Sets the variable on the test process itself, which every spawned child
/// inherits, so a `Command` helper that returns an owned `Command` still works.
pub fn isolate_recall() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_var(
            "RTK_RECALL_DB",
            std::env::temp_dir().join("rtk-integration-recall.db"),
        );
    });
}

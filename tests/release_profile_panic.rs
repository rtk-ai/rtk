//! Regression guard for the release panic strategy.
//!
//! rtk's filter execution fails open via `std::panic::catch_unwind`
//! (`core::stream`, `cmds::system::pipe_cmd`): when a filter panics, rtk is
//! supposed to print a warning and pass the raw output through untouched.
//!
//! `catch_unwind` only works under `panic = "unwind"`. If the release profile
//! is built with `panic = "abort"`, that recovery becomes dead code in the
//! shipped binary — any panic inside a filter aborts the whole process with
//! SIGABRT instead of degrading to raw passthrough. The unit tests for the
//! fail-open behavior still pass because `cargo test` builds with unwind, so
//! the regression is invisible to the normal test suite. This test inspects the
//! manifest directly so a reintroduced `panic = "abort"` fails CI.
//!
//! See: https://github.com/rtk-ai/rtk/issues/2400

#[test]
fn release_profile_does_not_abort_on_panic() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("read Cargo.toml");
    let value: toml::Value = manifest.parse().expect("parse Cargo.toml");

    let panic_setting = value
        .get("profile")
        .and_then(|p| p.get("release"))
        .and_then(|r| r.get("panic"))
        .and_then(|v| v.as_str());

    // Either unset (defaults to unwind) or explicitly "unwind" is acceptable.
    // "abort" defeats the filter catch_unwind fail-open path.
    assert_ne!(
        panic_setting,
        Some("abort"),
        "[profile.release] panic = \"abort\" defeats filter catch_unwind fail-open \
         (filters would SIGABRT instead of passing through raw output). Use \"unwind\"."
    );
}

#![cfg(unix)]
//! A path search must not buffer the engine's whole stdout.
//!
//! Regression: `rtk rg <pattern> logs/` over a 30 GB tree emitted 4.6 GB of
//! matches, which rtk held twice (the `.output()` byte buffer plus its lossy
//! `String` copy). The resulting ~9 GB got the calling agent OOM-killed. Raw
//! retention is capped at `RAW_CAP`, so peak memory no longer scales with how
//! much the engine emits.

use std::io::Write;
use std::process::Command;

/// Enough match output that buffering it whole is unmistakable against the cap.
const MATCH_BYTES: usize = 64 << 20;

/// Address-space ceiling: comfortably above rtk's baseline plus a few copies of
/// the 10 MiB cap, far below two copies of `MATCH_BYTES`.
const VM_LIMIT_KB: usize = 400_000;

#[test]
fn path_search_does_not_buffer_whole_engine_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let line = format!("{}MATCHME{}\n", "x".repeat(40), "y".repeat(40));
    {
        let mut w = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for _ in 0..(MATCH_BYTES / line.len()) {
            w.write_all(line.as_bytes()).unwrap();
        }
        w.flush().unwrap();
    }

    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "ulimit -v {}; exec {} grep MATCHME {}",
            VM_LIMIT_KB,
            env!("CARGO_BIN_EXE_rtk"),
            path.display()
        ))
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "rtk grep died under a {} KB address-space cap: {:?}\nstderr: {}",
        VM_LIMIT_KB,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("matches in"),
        "expected the capped grouped form, got: {}",
        &stdout[..stdout.len().min(400)]
    );
}

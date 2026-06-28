# Design Document: Short-Output Auto-Passthrough

> **Issue**: [#2673](https://github.com/rtk-ai/rtk/issues/2673)
> **Complements**: [#2121](https://github.com/rtk-ai/rtk/issues/2121) / [PR #2667](https://github.com/rtk-ai/rtk/pull/2667) (hook-level signal passthrough)
> **Author**: @lg320531124
> **Date**: 2026-06-28

---

## 1. Problem Statement

RTK compresses **all** command output through its reduction pipeline. When output is already short (≤5 lines / ≤500 bytes), compression yields zero token savings and can actively harm downstream agent decisions by reshaping or inflating canonical markers.

PR #2667 adds hook-level passthrough for predefined signal commands (`git push`, `gh pr`, etc.), but:

1. **Only covers the hook layer** — `rtk git status` called directly still compresses
2. **Allowlist-based** — `echo $?`, `which python3`, `hostname` etc. aren't in the list
3. **Coarse heuristic** — `git status -uall` (1000 lines) gets the same passthrough as clean `git status` (3 lines)

We need an **output-length heuristic** at the CLI execution layer that auto-passthroughs any short output, regardless of command name.

---

## 2. Architecture

### 2.1 Current Flow (without auto-passthrough)

```
Command enters RTK
  → main.rs routes to *_cmd.rs
  → *_cmd.rs calls runner::run_filtered() or run_filtered_with_exit()
  → run_captured_filter():
      1. stream::run_streaming() captures raw output
      2. filter_fn(text_to_filter, exit_code) compresses output
      3. guard::never_worse() ensures filtered ≤ raw
      4. print filtered output
      5. timer.track() records token savings
```

### 2.2 New Flow (with auto-passthrough)

```
Command enters RTK
  → main.rs routes to *_cmd.rs
  → *_cmd.rs calls runner::run_filtered() or run_filtered_with_exit()
  → run_captured_filter():
      1. stream::run_streaming() captures raw output
      2. ★ NEW: should_auto_passthrough(text_to_filter) ★
         → YES: print raw output, timer.track_passthrough(), return early
         → NO:  continue to filter_fn
      3. filter_fn(text_to_filter, exit_code) compresses output
      4. guard::never_worse() ensures filtered ≤ raw
      5. print filtered output
      6. timer.track() records token savings
```

The check goes **after** capture but **before** filtering. This is the natural insertion point because:

- Raw output is already available (captured by `run_streaming`)
- No filter pipeline work has been done yet (zero wasted effort)
- `skip_filter_on_failure` check happens first (failure passthrough takes precedence)

---

## 3. Design Details

### 3.1 `PassthroughConfig` — New Config Section

Location: `src/core/config.rs`

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PassthroughConfig {
    /// Auto-passthrough when output has ≤ N lines.
    /// 0 = disabled (current behavior). Default: 5.
    #[serde(default)]
    pub short_line_threshold: usize,

    /// Auto-passthrough when output has ≤ N bytes.
    /// 0 = disabled (current behavior). Default: 500.
    #[serde(default)]
    pub short_byte_threshold: usize,
}
```

Add to `Config` struct:

```rust
pub struct Config {
    // ... existing fields ...
    #[serde(default)]
    pub passthrough: PassthroughConfig,
}
```

**Why `usize` not `Option<usize>`**: `0` means disabled, which is the current behavior. No need for `Option` — simpler API, simpler TOML.

**Why not `PassthroughConfig::default()` with values**: If we set defaults in the `Default` impl, then `Config::load()` for existing config.toml files (without `[passthrough]` section) will get `0/0` from serde's default. Instead, `should_auto_passthrough()` will use hardcoded defaults when thresholds are 0.

**Config file example**:

```toml
# ~/.config/rtk/config.toml
[passthrough]
short_line_threshold = 5    # ≤ 5 lines → passthrough
short_byte_threshold = 500  # ≤ 500 bytes → passthrough
```

### 3.2 `should_auto_passthrough()` — Core Heuristic

Location: `src/core/runner.rs`

```rust
/// Whether the raw output is short enough that compression adds no value.
/// When true, the caller should emit the raw output unchanged.
fn should_auto_passthrough(output: &str) -> bool {
    let config = crate::core::config::Config::load().ok();

    // Hardcoded defaults: 5 lines, 500 bytes.
    // Config values of 0 mean "disabled" (current behavior).
    let line_threshold = config
        .as_ref()
        .map(|c| c.passthrough.short_line_threshold)
        .filter(|&t| t > 0)
        .unwrap_or(5);
    let byte_threshold = config
        .as_ref()
        .map(|c| c.passthrough.short_byte_threshold)
        .filter(|&t| t > 0)
        .unwrap_or(500);

    let line_count = output.lines().count();
    line_count <= line_threshold && output.len() <= byte_threshold
}
```

**Both conditions must be met** (AND, not OR):
- `line_count <= 5`: Covers `git push` (2 lines), `echo $?` (1 line), clean `git status` (3 lines)
- `output.len() <= 500`: Prevents passthrough for very long single-line output (e.g., minified JSON)

### 3.3 Integration Point in `run_captured_filter()`

Location: `src/core/runner.rs`, function `run_captured_filter()`

After line 129 (`let text_to_filter = ...`), before line 130 (`let filtered = filter_fn(...)`):

```rust
    let text_to_filter = if opts.filter_stdout_only {
        raw_stdout
    } else {
        raw
    };

    // ★ NEW: Short-output auto-passthrough
    if should_auto_passthrough(text_to_filter) {
        if opts.no_trailing_newline {
            print!("{}", text_to_filter);
        } else {
            println!("{}", text_to_filter);
        }
        // Emit stderr if it exists and wasn't already printed
        if !opts.filter_stdout_only && !result.raw_stderr.trim().is_empty() {
            eprint!("{}", result.raw_stderr);
        }
        timer.track_passthrough(cmd_label, &format!("rtk {} (auto-passthrough)", cmd_label));
        return Ok(exit_code);
    }

    let filtered = filter_fn(text_to_filter, exit_code);
```

**Key decisions**:

1. **Respects `no_trailing_newline`**: Same logic as the normal path (line 142-146)
2. **stderr handling**: If `filter_stdout_only` is false, we're filtering combined stdout+stderr. Passthrough should also include stderr. If `filter_stdout_only` is true, stderr is handled separately by the caller.
3. **Track as passthrough**: Uses `timer.track_passthrough()` which records 0 tokens saved — honest about the fact that no compression happened.
4. **After `skip_filter_on_failure`**: If the command failed and `skip_filter_on_failure` is set, that check (line 114) takes precedence.

### 3.4 Why Not in `RunMode::Streamed`

Streamed commands pipe output through a `StreamFilter` incrementally — they can't know the total size until the stream ends. Most short-output commands use `RunMode::Filtered` (captured mode), which already buffers the full output. The heuristic only applies to captured mode.

`RunMode::Passthrough` already handles its own case (no filtering at all).

### 3.5 Interaction with Existing Features

| Feature | Interaction |
|---------|-------------|
| **`skip_filter_on_failure`** | Takes precedence — checked first in `run_captured_filter()` |
| **`guard::never_worse()`** | Not reached — passthrough returns before filtering |
| **`tee_label`** | Not reached — passthrough returns before tee. TODO: consider teeing before passthrough in future PR |
| **`filter_stdout_only`** | Respected — `text_to_filter` is already correctly scoped |
| **`no_trailing_newline`** | Respected — same print logic as normal path |
| **PR #2667 (hook passthrough)** | Complementary — #2667 prevents hook rewrite; this prevents CLI compression |

### 3.6 `git status -uall` Edge Case

`git status -uall` with 1000 untracked files:
- **Hook layer (#2667)**: `git status` matches signal pattern → hook doesn't rewrite → raw `git status -uall` runs → 1000 lines of output
- **CLI layer (this PR)**: `rtk git status -uall` → 1000 lines > 5-line threshold → compressed as normal ✅

This is a **feature**, not a bug: the output-length heuristic is finer-grained than command-name matching.

---

## 4. Test Plan

### 4.1 Unit Tests (in `src/core/runner.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_auto_passthrough_short_output() {
        // ≤5 lines, ≤500 bytes → passthrough
        assert!(should_auto_passthrough("ok\n"));
        assert!(should_auto_passthrough("line1\nline2\nline3\n"));
        assert!(should_auto_passthrough("To github.com:user/repo.git\n   abc..def main -> main\n"));
    }

    #[test]
    fn test_should_auto_passthrough_long_output() {
        // >5 lines → no passthrough
        let six_lines = "1\n2\n3\n4\n5\n6\n";
        assert!(!should_auto_passthrough(six_lines));
    }

    #[test]
    fn test_should_auto_passthrough_large_bytes() {
        // ≤5 lines but >500 bytes → no passthrough
        let long_line = format!("{}\n", "x".repeat(501));
        assert!(!should_auto_passthrough(&long_line));
    }

    #[test]
    fn test_should_auto_passthrough_empty_output() {
        // 0 lines, 0 bytes → passthrough (nothing to compress)
        assert!(should_auto_passthrough(""));
    }

    #[test]
    fn test_should_auto_passthrough_exact_threshold() {
        // Exactly 5 lines, ≤500 bytes → passthrough
        let five_lines = "1\n2\n3\n4\n5\n";
        assert!(should_auto_passthrough(five_lines));

        // 6 lines → no passthrough
        let six_lines = "1\n2\n3\n4\n5\n6\n";
        assert!(!should_auto_passthrough(six_lines));
    }

    #[test]
    fn test_should_auto_passthrough_disabled() {
        // When config has threshold = 0, should_auto_passthrough uses defaults (5/500)
        // To fully disable, user would need to set thresholds to 0 and
        // we'd need a separate "disabled" flag.
        // For now, 0/0 → defaults kick in. Document this.
    }
}
```

### 4.2 Config Deserialization Tests (in `src/core/config.rs`)

```rust
#[test]
fn test_passthrough_config_default() {
    let config = Config::default();
    assert_eq!(config.passthrough.short_line_threshold, 0);
    assert_eq!(config.passthrough.short_byte_threshold, 0);
}

#[test]
fn test_passthrough_config_deserialize() {
    let toml = r#"
[passthrough]
short_line_threshold = 3
short_byte_threshold = 200
"#;
    let config: Config = toml::from_str(toml).expect("valid toml");
    assert_eq!(config.passthrough.short_line_threshold, 3);
    assert_eq!(config.passthrough.short_byte_threshold, 200);
}

#[test]
fn test_passthrough_config_missing_section() {
    // Old configs without [passthrough] must still parse
    let toml = r#"
[tracking]
enabled = true
"#;
    let config: Config = toml::from_str(toml).expect("valid toml");
    assert_eq!(config.passthrough.short_line_threshold, 0);
    assert_eq!(config.passthrough.short_byte_threshold, 0);
}
```

### 4.3 Integration Test

```bash
# Verify short output passes through unchanged
echo "hello" > /tmp/short.txt
rtk cat /tmp/short.txt
# Expected: "hello" (unmodified, no rtk prefix or formatting)

# Verify long output still gets compressed
rtk git log -50
# Expected: compressed output with [git: log] header
```

---

## 5. Files Changed

| File | Change | Lines |
|------|--------|-------|
| `src/core/config.rs` | Add `PassthroughConfig` struct + field on `Config` + deserialize test | ~30 |
| `src/core/runner.rs` | Add `should_auto_passthrough()` + integration in `run_captured_filter()` + tests | ~60 |
| **Total** | | **~90** |

No new files, no new modules, no new dependencies.

---

## 6. Performance Impact

- **Short-output commands**: Skip the entire filter pipeline → *faster* than current behavior
- **Long-output commands**: `should_auto_passthrough()` adds one `lines().count()` + one `len()` call on already-buffered output → negligible (<0.1ms)
- **Config load**: `Config::load()` is already called per invocation in other places. The call in `should_auto_passthrough()` could be cached, but the cost is already amortized across the process lifetime.

---

## 7. Migration / Backward Compatibility

- **Default behavior**: Thresholds default to `0/0` in config struct, but `should_auto_passthrough()` uses `5/500` as runtime defaults. Existing users get auto-passthrough without changing their config.
- **Opt-out**: Set both thresholds to 0 and add a `passthrough_enabled = false` flag? No — simpler: document that setting `short_line_threshold = 0` disables auto-passthrough.
- **Config format**: `[passthrough]` is a new section. Missing section → `Default::default()` → thresholds are `0` → `should_auto_passthrough()` uses runtime defaults. This is backward-compatible.

Wait — this is a design tension. Let me resolve it:

**Option A**: Config defaults `0/0` → runtime defaults `5/500` → auto-passthrough ON by default
- Pro: Users get the benefit immediately
- Con: Existing users may not expect behavior change; `0` in config means "use defaults" not "disabled"

**Option B**: Config defaults `5/500` → runtime uses config values directly
- Pro: Explicit; `0` means "0 lines → nothing passes through → disabled"
- Con: `Config::default()` would enable auto-passthrough in tests

**Chosen: Option B** with explicit defaults via `effective_*()` methods:

```rust
impl PassthroughConfig {
    pub fn effective_line_threshold(&self) -> usize {
        if self.short_line_threshold > 0 { self.short_line_threshold } else { 5 }
    }
    pub fn effective_byte_threshold(&self) -> usize {
        if self.short_byte_threshold > 0 { self.short_byte_threshold } else { 500 }
    }
}
```

This means:
- New installations: auto-passthrough ON (5 lines / 500 bytes)
- Existing config.toml without `[passthrough]`: `Config::load()` → serde fills defaults → `0/0` → `effective_*()` returns 5/500 → ON
- To opt out: set `short_line_threshold = 1` + `short_byte_threshold = 1` → only empty output passes through

---

## 8. Open Questions

1. **Tee integration**: When auto-passthrough triggers, should we still tee the raw output for CCR (#2485)? Currently tee is not reached. A future PR could add tee-before-passthrough.

2. **Verbose logging**: Should `rtk -v git status` print "auto-passthrough: 3 lines / 120 bytes (≤5/500)"? Useful for debugging but adds noise. Suggest: only with `-vv`.

3. **Hook layer coordination**: When #2667 is merged, the hook will skip rewriting `git status`. But if the user calls `rtk git status` directly, our auto-passthrough kicks in. Should we emit a log message when auto-passthrough triggers for a command that's also in the hook signal list? Probably not — that's implementation detail.

4. **Threshold tuning**: The 5-line / 500-byte defaults are conservative. Real-world data from `rtk gain` could inform better defaults. But starting conservative is safer — we can always lower thresholds later.

---

## 9. Implementation Checklist

- [ ] Add `PassthroughConfig` to `src/core/config.rs` with `Default` impl
- [ ] Add `passthrough` field to `Config` struct
- [ ] Add `should_auto_passthrough()` to `src/core/runner.rs`
- [ ] Integrate check into `run_captured_filter()` after capture, before filter
- [ ] Add config deserialize tests
- [ ] Add unit tests for `should_auto_passthrough()`
- [ ] Run `cargo fmt --all && cargo clippy --all-targets && cargo test --all`
- [ ] Test manually: `rtk git status` (short → passthrough) vs `rtk git log -50` (long → compressed)
- [ ] Commit with message: `feat: short-output auto-passthrough (#2673)`
- [ ] Push to feature branch, open PR targeting `develop`

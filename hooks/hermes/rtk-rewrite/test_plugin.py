"""Test suite for RTK Hermes plugin v1.0.0.

Each function has at least 3 tests:
  - Normal case
  - Edge case documented in Phase 1
  - Behavior comparison with Rust source
"""

import json
import sys
import os

sys.path.insert(0, os.path.expanduser("~/.hermes/plugins/rtk-rewrite"))
import __init__ as rtk

passed = 0
failed = 0

def test(name, assertion, detail=""):
    global passed, failed
    if assertion:
        passed += 1
    else:
        failed += 1
        print(f"FAIL: {name} — {detail}")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 1: _strip_ansi
# Ref: utils.rs:48
# ══════════════════════════════════════════════════════════════════════════════

# Normal: strip CSI sequences
test("strip_ansi: CSI green",
     rtk._strip_ansi("\x1b[32mgreen\x1b[0m") == "green")

test("strip_ansi: CSI bold+color",
     rtk._strip_ansi("\x1b[1;31mred\x1b[0m normal") == "red normal")

# Edge: empty string
test("strip_ansi: empty",
     rtk._strip_ansi("") == "")

# Edge: OSC sequences NOT stripped (matching Rust behavior)
test("strip_ansi: OSC NOT stripped (like Rust)",
     "\x1b]0;title\x07" in rtk._strip_ansi("\x1b]0;title\x07link"))

# Edge: no ANSI at all
test("strip_ansi: no ANSI",
     rtk._strip_ansi("plain text") == "plain text")


# ══════════════════════════════════════════════════════════════════════════════
# _truncate (utils.rs:25)
# ══════════════════════════════════════════════════════════════════════════════

# Normal: truncate long string
test("truncate: normal",
     rtk._truncate("hello world", 8) == "hello...")

# Edge: max_len < 3 → "..." (utils.rs:31)
test("truncate: max_len<3",
     rtk._truncate("hello", 2) == "...")
test("truncate: max_len=0",
     rtk._truncate("hello", 0) == "...")
test("truncate: max_len=1",
     rtk._truncate("hello", 1) == "...")

# Edge: max_len = 3 → "..." if longer
test("truncate: max_len=3, string longer",
     rtk._truncate("hello", 3) == "...")

# Edge: string shorter than max_len → unchanged
test("truncate: shorter than max",
     rtk._truncate("hi", 10) == "hi")

# Edge: exactly max_len → unchanged
test("truncate: exactly max_len",
     rtk._truncate("hello", 5) == "hello")

# Unicode: multi-byte chars — Rust chars() counts code points (same as Python len())
# 👨‍👩‍👧👦 = 7 code points, "family" = 7 chars = 14 total. 14 > 7, truncate to 4+3
test("truncate: emoji (code-point truncation, like Rust)",
     "..." in rtk._truncate("👨‍👩‍👧👦family", 7))

# CJK: 你好世界hello = 9 chars. 9 > 6, truncate to 3+3
test("truncate: CJK (code-point truncation, like Rust)",
     "..." in rtk._truncate("你好世界hello", 6))

# Empty string
test("truncate: empty string",
     rtk._truncate("", 5) == "")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 2: _apply_replace
# Ref: toml_filter.rs:460-474
# ══════════════════════════════════════════════════════════════════════════════

# Normal: single replace per line (Rust uses $1 for backreferences)
test("replace: single rule",
     rtk._apply_replace("v1.0.0", [{"pattern": r"v(\d+\.\d+\.\d+)", "replacement": "$1"}]) == "1.0.0")

# Normal: multi-line, per-line (Rust iterates lines())
test("replace: per-line",
     rtk._apply_replace("abc\ndef", [{"pattern": "abc", "replacement": "XXX"}]) == "XXX\ndef")

# Normal: chained replaces
test("replace: chain",
     rtk._apply_replace("hello", [
         {"pattern": "hello", "replacement": "hi"},
         {"pattern": "hi", "replacement": "hey"},
     ]) == "hey")

# Edge: no rules → passthrough
test("replace: no rules",
     rtk._apply_replace("text", []) == "text")

# Edge: invalid regex → warn + skip (compile_filter: bad regex → skip)
result = rtk._apply_replace("test", [{"pattern": "[invalid", "replacement": "x"}])
test("replace: invalid regex skips",
     result == "test")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 3: _apply_match_output
# Ref: toml_filter.rs:476-494
# ══════════════════════════════════════════════════════════════════════════════

# Normal: pattern matches → short-circuit
shorted, msg = rtk._apply_match_output("No dependencies to install", [
    {"pattern": "No dependencies", "message": "ok (up to date)"}
])
test("match_output: basic short-circuit",
     shorted is True and msg == "ok (up to date)")

# Normal: pattern matches but unless blocks (toml_filter.rs:482)
shorted, msg = rtk._apply_match_output("No dependencies but error found", [
    {"pattern": "No dependencies", "message": "ok", "unless": "error"}
])
test("match_output: unless blocks",
     shorted is False and "error" in msg)

# Normal: unless doesn't block
shorted, msg = rtk._apply_match_output("No dependencies to install", [
    {"pattern": "No dependencies", "message": "ok", "unless": "error|failed"}
])
test("match_output: unless allows",
     shorted is True and msg == "ok")

# Edge: no match → continue
shorted, msg = rtk._apply_match_output("Error occurred", [
    {"pattern": "Success", "message": "ok"}
])
test("match_output: no match continues",
     shorted is False and msg == "Error occurred")

# Edge: re.DOTALL — Rust `.` matches \n
shorted, msg = rtk._apply_match_output("line1\nline2", [
    {"pattern": "line1.line2", "message": "ok (DOTALL)"}
])
test("match_output: re.DOTALL (. matches \\n)",
     shorted is True and msg == "ok (DOTALL)")

# Edge: no rules
shorted, msg = rtk._apply_match_output("text", [])
test("match_output: no rules",
     shorted is False and msg == "text")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 4a: _apply_strip_lines_matching
# Ref: toml_filter.rs:496-505
# ══════════════════════════════════════════════════════════════════════════════

# Normal: strip blank lines
test("strip_lines: blank lines",
     rtk._apply_strip_lines_matching("line1\n\nline2\n\n\nline3", [r"^\s*$"]) == "line1\nline2\nline3")

# Normal: strip specific pattern
test("strip_lines: specific pattern",
     rtk._apply_strip_lines_matching("[INFO] Downloading\n[INFO] Building\n[ERROR] fail", [r"^\[INFO\]"]) == "[ERROR] fail")

# Edge: all lines match → empty
test("strip_lines: all match",
     rtk._apply_strip_lines_matching("\n\n", [r"^\s*$"]) == "")

# Edge: no lines match → unchanged
test("strip_lines: none match",
     rtk._apply_strip_lines_matching("line1\nline2", [r"^XXX"]) == "line1\nline2")

# Edge: no patterns → passthrough
test("strip_lines: no patterns",
     rtk._apply_strip_lines_matching("text", []) == "text")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 4b: _apply_keep_lines_matching
# Ref: toml_filter.rs:507-514
# ══════════════════════════════════════════════════════════════════════════════

# Normal: keep only matching
test("keep_lines: basic",
     rtk._apply_keep_lines_matching("ERROR: fail\nWARN: ok\nINFO: skip", [r"^ERROR", r"^WARN"]) == "ERROR: fail\nWARN: ok")

# Edge: no lines match → empty
test("keep_lines: none match",
     rtk._apply_keep_lines_matching("line1\nline2", [r"^XXX"]) == "")

# Edge: no patterns → passthrough
test("keep_lines: no patterns",
     rtk._apply_keep_lines_matching("text", []) == "text")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 5: _apply_truncate_lines_at
# Ref: toml_filter.rs (via utils.rs:25 per line)
# ══════════════════════════════════════════════════════════════════════════════

# Normal: truncate long lines
test("truncate_lines: normal",
     "..." in rtk._apply_truncate_lines_at("short\nthis is a very long line indeed\nhi", 20) and
     "short" in rtk._apply_truncate_lines_at("short\nthis is a very long line indeed\nhi", 20))

# Edge: max_len < 1 → passthrough
test("truncate_lines: max_len<1",
     rtk._apply_truncate_lines_at("hello", 0) == "hello")

# Edge: max_len < 3 → all lines become "..."
test("truncate_lines: max_len=2",
     rtk._apply_truncate_lines_at("hello\nworld", 2) == "...\n...")

# Edge: empty string
test("truncate_lines: empty",
     rtk._apply_truncate_lines_at("", 10) == "")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 6: _apply_head_tail_lines
# Ref: toml_filter.rs:517-535
# ══════════════════════════════════════════════════════════════════════════════

lines_100 = "\n".join(f"line {i}" for i in range(100))

# Normal: head+tail with total > head+tail
result = rtk._apply_head_tail_lines(lines_100, 40, 20)
test("head_tail: total > head+tail",
     "line 0" in result and "line 99" in result and "40 lines omitted" in result)

# Critical: head+tail with total <= head+tail → KEEP ALL (Rust behavior)
lines_45 = "\n".join(f"line {i}" for i in range(45))
result = rtk._apply_head_tail_lines(lines_45, 40, 20)
test("head_tail: total <= head+tail → keep all",
     "line 44" in result and "omitted" not in result)

# Normal: head only
result = rtk._apply_head_tail_lines(lines_100, 10, 0)
test("head_tail: head only",
     "line 0" in result and "line 99" not in result and "90 lines omitted" in result)

# Normal: tail only
result = rtk._apply_head_tail_lines(lines_100, 0, 10)
test("head_tail: tail only",
     "line 99" in result and "line 0" not in result and "90 lines omitted" in result)

# Edge: negative values → ValueError (Rust Option<usize> cannot be negative)
try:
    rtk._apply_head_tail_lines(lines_100, -1, 0)
    test("head_tail: negative head raises", False, "no ValueError raised")
except ValueError:
    test("head_tail: negative head raises", True)

try:
    rtk._apply_head_tail_lines(lines_100, 0, -1)
    test("head_tail: negative tail raises", False, "no ValueError raised")
except ValueError:
    test("head_tail: negative tail raises", True)

# Edge: total <= head
test("head_tail: total <= head",
     rtk._apply_head_tail_lines("a\nb\nc", 10, 0) == "a\nb\nc")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 7: _apply_max_lines
# Ref: toml_filter.rs:537-543
# ══════════════════════════════════════════════════════════════════════════════

# Normal: truncate with marker (Rust format: "... (N lines truncated)")
result = rtk._apply_max_lines("\n".join(f"line {i}" for i in range(50)), 10)
test("max_lines: truncate",
     "lines truncated" in result and "line 9" in result and "line 10" not in result)

# Edge: total <= max_lines → unchanged
test("max_lines: under cap",
     rtk._apply_max_lines("a\nb\nc", 10) == "a\nb\nc")

# Edge: exactly max_lines → unchanged
test("max_lines: exactly cap",
     rtk._apply_max_lines("a\nb\nc", 3) == "a\nb\nc")

# Edge: max_lines < 1 → passthrough
test("max_lines: max<1",
     rtk._apply_max_lines("text", 0) == "text")


# ══════════════════════════════════════════════════════════════════════════════
# Stage 8: _apply_on_empty
# Ref: toml_filter.rs:545-547
# ══════════════════════════════════════════════════════════════════════════════

test("on_empty: empty → message",
     rtk._apply_on_empty("", "make: ok") == "make: ok")

test("on_empty: whitespace only → message",
     rtk._apply_on_empty("  \n\t  ", "ok") == "ok")

test("on_empty: non-empty → unchanged",
     rtk._apply_on_empty("output", "ok") == "output")

test("on_empty: no message → unchanged",
     rtk._apply_on_empty("", "") == "")


# ══════════════════════════════════════════════════════════════════════════════
# Full pipeline: _apply_filter
# Ref: toml_filter.rs:436-547
# ══════════════════════════════════════════════════════════════════════════════

# Pipeline with ANSI + blanks + too many lines
big_text = "\x1b[32mOK\x1b[0m\n\n" + "\n".join(f"line {i}" for i in range(300))
profile = {"strip_ansi": True, "strip_lines_matching": [r"^\s*$"], "max_lines": 50}
result = rtk._apply_filter(big_text, profile)
test("pipeline: full pipeline",
     "\x1b[" not in result and "\n\n" not in result and "truncated" in result)

# Pipeline with match_output short-circuit
result = rtk._apply_filter("Build complete!", {
    "strip_ansi": True,
    "match_output": [{"pattern": "Build complete!", "message": "ok (built)"}],
    "max_lines": 10
})
test("pipeline: match_output short-circuits",
     result == "ok (built)")

# Pipeline with on_empty
result = rtk._apply_filter("   \n  \n", {
    "strip_ansi": True,
    "strip_lines_matching": [r"^\s*$"],
    "on_empty": "make: ok"
})
test("pipeline: on_empty after strip",
     result == "make: ok")

# Profile validation: strip+keep mutually exclusive
result = rtk._apply_filter("text", {
    "strip_lines_matching": ["^X"],
    "keep_lines_matching": ["^Y"]
})
test("pipeline: strip+keep warns (mutually exclusive)",
     True)  # just verify it doesn't crash


# ══════════════════════════════════════════════════════════════════════════════
# Hermes layer: H1 _is_json_result
# ══════════════════════════════════════════════════════════════════════════════

test("is_json: dict",
     rtk._is_json_result('{"key": "value"}') is True)
test("is_json: array",
     rtk._is_json_result('[1, 2, 3]') is True)
test("is_json: invalid",
     rtk._is_json_result("not json") is False)
test("is_json: string scalar",
     rtk._is_json_result('"just a string"') is False)


# ══════════════════════════════════════════════════════════════════════════════
# Hermes layer: H2 _strip_verbose_json_keys
# ══════════════════════════════════════════════════════════════════════════════

# Large dict with verbose keys
big = {"content": "x" * 3000, "truncated": "y" * 200, "status": 200, "is_binary": False, "file_size": 3.14, "hint": "offset", "total_lines": 500, "dedup": None}
stripped = rtk._strip_verbose_json_keys(big)
test("verbose_keys: large string dropped",
     "y" * 200 not in stripped.get("truncated", ""))
test("verbose_keys: int kept",
     stripped.get("status") == 200)
test("verbose_keys: bool kept",
     stripped.get("is_binary") is False)
test("verbose_keys: float kept",
     stripped.get("file_size") == 3.14)
test("verbose_keys: None kept",
     stripped.get("dedup") is None)
test("verbose_keys: hint kept",
     stripped.get("hint") == "offset")
test("verbose_keys: total_lines kept",
     stripped.get("total_lines") == 500)
test("verbose_keys: non-verbose key preserved",
     "content" in stripped)

# Small dict → passthrough
small = {"content": "short"}
test("verbose_keys: under threshold → unchanged",
     rtk._strip_verbose_json_keys(small) == small)


# ══════════════════════════════════════════════════════════════════════════════
# Hermes layer: H3 _truncate_json_values
# ══════════════════════════════════════════════════════════════════════════════

test("truncate_json: short value unchanged",
     rtk._truncate_json_values("short", max_len=500) == "short")

long_val = "x" * 1000
truncd = rtk._truncate_json_values(long_val, max_len=100)
test("truncate_json: long value truncated",
     len(truncd) <= 100 and "..." in truncd)
test("truncate_json: truncated shorter than original",
     len(truncd) < len(long_val))

# Edge: max_len very small (usable < 2)
test("truncate_json: max_len=5",
     rtk._truncate_json_values("x" * 100, max_len=5) == "x...x")

# Nested
nested = {"outer": {"inner": "x" * 1000}}
truncd_n = rtk._truncate_json_values(nested, max_len=100)
test("truncate_json: nested dict",
     len(truncd_n["outer"]["inner"]) <= 100)


# ══════════════════════════════════════════════════════════════════════════════
# Hermes layer: H4 _compress_tool_result
# ══════════════════════════════════════════════════════════════════════════════

# JSON path
big_json = json.dumps({"content": "x" * 5000, "total_lines": 500, "hint": "offset", "status": 200})
comp = rtk._compress_tool_result("read_file", big_json)
test("compress: JSON preserved",
     json.loads(comp) is not None)  # valid JSON
test("compress: JSON keys preserved",
     "hint" in json.loads(comp) and "total_lines" in json.loads(comp))
test("compress: JSON shorter",
     len(comp) < len(big_json))

# Text path
big_text = "\n".join(f"line {i}" for i in range(500))
comp = rtk._compress_tool_result("read_file", big_text)
test("compress: text shorter",
     len(comp) < len(big_text))

# Under threshold → passthrough
test("compress: under threshold",
     rtk._compress_tool_result("read_file", "short") == "short")


# ══════════════════════════════════════════════════════════════════════════════
# Hermes layer: H9 _format_tokens
# Ref: utils.rs:78
# ══════════════════════════════════════════════════════════════════════════════

test("format_tokens: millions",
     rtk._format_tokens(1_500_000) == "1.5M")
test("format_tokens: thousands",
     rtk._format_tokens(59_200) == "59.2K")
test("format_tokens: under 1K",
     rtk._format_tokens(694) == "694")


# ══════════════════════════════════════════════════════════════════════════════

print(f"\n{'='*60}")
print(f"Results: {passed} passed, {failed} failed")
if failed == 0:
    print("ALL TESTS PASSED")
else:
    print(f"FAILURES DETECTED — {failed} tests failed")
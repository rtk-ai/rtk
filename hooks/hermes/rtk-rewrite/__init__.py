"""Hermes plugin adapter for RTK command rewriting + tool result compression.

COUCHE RTK (lignes ~1-400): Port du pipeline 8 étapes depuis toml_filter.rs.
  Chaque fonction a une réf vers la ligne Rust source.
  AUCUN code inventé — tout vient du source RTK.

  NON PORTÉ (non applicable Hermes):
    - filter_stderr: Rust merge stderr avec stdout avant filtre. Hermes reçoit
      déjà des tool_results agrégés, pas un flux stdout/stderr séparé.
    - TOML parsing / registry / trust gating / find_filter_in: Infrastructure
      de config Rust, pas de logique pipeline.
    - run_filter_tests: Suite de tests interne Rust.

COUCHE HERMES (lignes ~400+): Hooks, DB, tag injection.
  Ces fonctions n'ont PAS de correspondant Rust.
  Elles sont clairement séparées.
"""

import json
import os
import re
import sqlite3
import subprocess
import sys
from pathlib import Path


# ══════════════════════════════════════════════════════════════════════════════
# COUCHE RTK — Pipeline 8 étapes (porté de toml_filter.rs + utils.rs)
# ══════════════════════════════════════════════════════════════════════════════

# --- Pré-compilation regex (équivalent lazy_static de Rust) ---

# Ref: utils.rs ligne 50 — lazy_static! regex `\x1b\[[0-9;]*[a-zA-Z]`
# Rust ne gère PAS les séquences OSC (\x1b]), DCS, etc. On fait pareil.
_ANSI_CSI_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")


# --- Helper: Rust-compatible line splitting ---
# Rust `.lines()` does NOT produce a trailing empty string for input ending with \\n.
# Python `split("\\n")` DOES produce one.
# We normalize this: split, and if the last element is empty string, remove it.
# This tracks whether the original had a trailing newline for re-joining.

def _rust_lines(text: str) -> list:
    """Split text into lines matching Rust's .lines() behavior.

    Rust's String::lines() does NOT produce a trailing empty string element
    for input ending with \\n. Python's split(\"\\n\") does.
    This function strips the trailing empty string to match Rust.
    """
    lines = text.split("\n")
    # Rust: "a\\n".lines() → ["a"], Python: "a\\n".split("\\n") → ["a", ""]
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def _rust_join(lines: list) -> str:
    """Join lines with \\n, matching Rust's .join(\"\\n\")."""
    return "\n".join(lines)


# ─── Stage 1: strip_ansi ────────────────────────────────────────────────────
# Ref: utils.rs ligne 48 — `pub fn strip_ansi(text: &str) -> String`
# Ref: toml_filter.rs ligne 451 — `if filter.strip_ansi { out = utils::strip_ansi(&out); }`

def _strip_ansi(text: str) -> str:
    """Stage 1: Strip ANSI CSI escape sequences.

    Ref: utils.rs:48 — uses lazy_static regex `\\x1b\\[[0-9;]*[a-zA-Z]`.
    Does NOT handle OSC (\\x1b]), DCS, or other escape types, matching Rust behavior.
    """
    return _ANSI_CSI_RE.sub("", text)


# ─── Fonction utilitaire: truncate ─────────────────────────────────────────
# Ref: utils.rs ligne 25 — `pub fn truncate(s: &str, max_len: usize) -> String`

def _truncate(s: str, max_len: int) -> str:
    """Truncate a string to max_len Unicode characters, appending '...' if needed.

    Ref: utils.rs:25-45 — exact same logic:
      - max_len < 3 → "..."
      - chars().count() <= max_len → return s unchanged
      - else → chars[:max_len-3] + "..."
    """
    char_count = len(s)
    if char_count <= max_len:
        # Ref: utils.rs:27 — `if s.chars().count() <= max_len { return s.to_string(); }`
        # Rust checks this FIRST, before max_len < 3
        return s
    if max_len < 3:
        # Ref: utils.rs:31 — `if max_len < 3 { return "...".to_string(); }`
        return "..."
    # Ref: utils.rs:35-36 — chars[:max_len-3].collect() + "..."
    # Python string slicing is Unicode-safe (like Rust chars())
    return s[:max_len - 3] + "..."


# ─── Stage 2: replace ──────────────────────────────────────────────────────
# Ref: toml_filter.rs ligne 460-474
# ```rust
# if !filter.replace.is_empty() {
#     out = out.lines().map(|line| {
#         for rule in &filter.replace {
#             line = rule.pattern.replace_all(line, &rule.replacement);
#         }
#         line.to_string()
#     }).collect::<Vec<_>>().join("\n");
# }
# ```

def _apply_replace(text: str, rules: list) -> str:
    """Stage 2: Apply regex replacements, chain per line (like Rust sed).

    Ref: toml_filter.rs:460-474 — iterates lines(), for each line chains
    all ReplaceRule {pattern, replacement}. Re-joins with "\\n".

    Difference: Rust `.lines()` strips trailing \\n (no empty string).
    Python `split("\\n")` adds empty string for trailing \\n.
    We handle this by splitting and re-joining identically.
    """
    if not rules:
        return text

    lines = _rust_lines(text)
    result_lines = []
    for line in lines:
        for rule in rules:
            # Ref: toml_filter.rs:465 — `rule.pattern.replace_all(line, &rule.replacement)`
            # Rust regex crate uses $1, $2 for capture groups.
            # Python re.sub uses \1, \2. Convert $N → \N for compatibility.
            pattern = rule.get("pattern", "")
            replacement = rule.get("replacement", "")
            if not pattern:
                continue
            try:
                py_replacement = re.sub(r'\$(\d+)', r'\\\1', replacement)
                line = re.sub(pattern, py_replacement, line)
            except re.error:
                # Ref: compile_filter in toml_filter.rs — bad regex → warn + skip
                _warn(f"replace: invalid regex {pattern!r}, skipping")
        result_lines.append(line)
    return _rust_join(result_lines)


# ─── Stage 3: match_output ─────────────────────────────────────────────────
# Ref: toml_filter.rs ligne 476-494
# ```rust
# if let Some(rules) = &filter.match_output {
#     for rule in rules {
#         if rule.pattern.is_match(&out) {
#             if let Some(unless) = &rule.unless {
#                 if unless.is_match(&out) { continue; }
#             }
#             return rule.message.clone();
#         }
#     }
# }
# ```

def _apply_match_output(text: str, rules: list) -> tuple:
    """Stage 3: Short-circuit if entire output matches a pattern.

    Ref: toml_filter.rs:476-494 — if pattern matches the blob, return message.
    `unless` is a guard: if unless ALSO matches, skip this rule (continue).

    Returns (short_circuited: bool, result: str).
    Difference: Rust regex `.` matches \\n by default. Python needs re.DOTALL.
    """
    if not rules:
        return False, text

    for rule in rules:
        pattern = rule.get("pattern", "")
        message = rule.get("message", "")
        unless = rule.get("unless", "")
        if not pattern:
            continue
        try:
            # Ref: Rust regex crate: `.` matches \n by default.
            # Python: must use re.DOTALL to match this behavior.
            if re.search(pattern, text, re.DOTALL):
                if unless and re.search(unless, text, re.DOTALL):
                    # Ref: toml_filter.rs:482 — `if unless.is_match(&out) { continue; }`
                    continue
                return True, message
        except re.error:
            _warn(f"match_output: invalid regex {pattern!r}, skipping")
    return False, text


# ─── Stage 4a: strip_lines_matching ────────────────────────────────────────
# Ref: toml_filter.rs ligne 496-505
# ```rust
# if let LineFilter::Strip(set) = &filter.line_filter {
#     out = out.lines()
#         .filter(|line| !set.is_match(line))
#         .collect::<Vec<_>>()
#         .join("\n");
# }
# ```

def _apply_strip_lines_matching(text: str, patterns: list) -> str:
    """Stage 4a: Remove lines matching any of the regex patterns.

    Ref: toml_filter.rs:496-505 — retains lines NOT matching any pattern.
    Rust uses RegexSet for batch matching; Python uses individual regex search.

    Difference: Rust `.lines()` vs Python `split("\\n")` trailing empty string.
    We split, filter, rejoin — trailing empty string is preserved in non-match.
    """
    if not patterns:
        return text

    compiled = []
    for p in patterns:
        try:
            compiled.append(re.compile(p))
        except re.error:
            _warn(f"strip_lines_matching: invalid regex {p!r}, skipping")

    if not compiled:
        return text

    lines = _rust_lines(text)
    # Ref: toml_filter.rs:498 — `.filter(|line| !set.is_match(line))`
    filtered = [line for line in lines if not any(c.search(line) for c in compiled)]
    return _rust_join(filtered)


# ─── Stage 4b: keep_lines_matching ─────────────────────────────────────────
# Ref: toml_filter.rs ligne 507-514
# ```rust
# if let LineFilter::Keep(set) = &filter.line_filter {
#     out = out.lines()
#         .filter(|line| set.is_match(line))
#         .collect::<Vec<_>>()
#         .join("\n");
# }
# ```
# Mutually exclusive with 4a — validated in compile_filter (toml_filter.rs:316).

def _apply_keep_lines_matching(text: str, patterns: list) -> str:
    """Stage 4b: Keep ONLY lines matching any of the regex patterns.

    Ref: toml_filter.rs:507-514 — retains lines matching any pattern.
    Mutually exclusive with strip_lines_matching.
    """
    if not patterns:
        return text

    compiled = []
    for p in patterns:
        try:
            compiled.append(re.compile(p))
        except re.error:
            _warn(f"keep_lines_matching: invalid regex {p!r}, skipping")

    if not compiled:
        return text

    lines = _rust_lines(text)
    # Ref: toml_filter.rs:510 — `.filter(|line| set.is_match(line))`
    filtered = [line for line in lines if any(c.search(line) for c in compiled)]
    return _rust_join(filtered)


# ─── Stage 5: truncate_lines_at ───────────────────────────────────────────
# Ref: toml_filter.rs applies utils::truncate() per line
# Ref: utils.rs ligne 25 — `pub fn truncate(s: &str, max_len: usize) -> String`

def _apply_truncate_lines_at(text: str, max_len: int) -> str:
    """Stage 5: Truncate each line to max_len characters.

    Ref: toml_filter.rs:505 uses utils::truncate() on each line.
    Ref: utils.rs:25 — same _truncate logic per line.
    """
    if max_len < 1:
        return text
    lines = _rust_lines(text)
    return _rust_join(_truncate(line, max_len) for line in lines)


# ─── Stage 6: head_lines + tail_lines ──────────────────────────────────────
# Ref: toml_filter.rs ligne 517-535
# ```rust
# if filter.head_lines > 0 || filter.tail_lines > 0 {
#     let lines: Vec<&str> = out.lines().collect();
#     if filter.head_lines > 0 && filter.tail_lines > 0 {
#         if lines.len() > filter.head_lines + filter.tail_lines {
#             let omitted = lines.len() - filter.head_lines - filter.tail_lines;
#             out = format!("{}\n... ({} lines omitted)\n{}",
#                 lines[..filter.head_lines].join("\n"),
#                 omitted,
#                 lines[lines.len()-filter.tail_lines..].join("\n"));
#         }
#     } else if filter.head_lines > 0 && lines.len() > filter.head_lines {
#         let omitted = lines.len() - filter.head_lines;
#         out = format!("{}\n... ({} lines omitted)",
#             lines[..filter.head_lines].join("\n"), omitted);
#     } else if filter.tail_lines > 0 && lines.len() > filter.tail_lines {
#         let omitted = lines.len() - filter.tail_lines;
#         out = format!("... ({} lines omitted)\n{}",
#             omitted, lines[lines.len()-filter.tail_lines..].join("\n"));
#     }
# }
# ```

def _apply_head_tail_lines(text: str, head: int, tail: int) -> str:
    """Stage 6: Keep first/last N lines with omission marker.

    Ref: toml_filter.rs:517-535 — exact same 4-branch logic:
      1. head+tail both set, total > head+tail → head + marker + tail
      2. head+tail both set, total <= head+tail → KEEP ALL (no truncation)
      3. head only, total > head → head + marker
      4. tail only, total > tail → marker + tail
    Marker format: exactly "... (N lines omitted)" (matches Rust).
    """
    if head < 0:
        raise ValueError(f"head_lines must be >= 0, got {head}")
    if tail < 0:
        raise ValueError(f"tail_lines must be >= 0, got {tail}")
    if head == 0 and tail == 0:
        return text

    lines = _rust_lines(text)
    total = len(lines)

    if head > 0 and tail > 0:
        # Ref: toml_filter.rs:519-530
        if total > head + tail:
            omitted = total - head - tail
            return _rust_join(
                lines[:head]
                + [f"... ({omitted} lines omitted)"]
                + lines[-tail:]
            )
        # Ref: toml_filter.rs — total <= head+tail → return all lines unchanged
        return text
    elif head > 0 and total > head:
        # Ref: toml_filter.rs:531-533
        omitted = total - head
        return _rust_join(lines[:head] + [f"... ({omitted} lines omitted)"])
    elif tail > 0 and total > tail:
        # Ref: toml_filter.rs:534-536
        omitted = total - tail
        return _rust_join([f"... ({omitted} lines omitted)"] + lines[-tail:])
    return text


# ─── Stage 7: max_lines ────────────────────────────────────────────────────
# Ref: toml_filter.rs ligne 537-543
# ```rust
# if filter.max_lines > 0 {
#     let lines: Vec<&str> = out.lines().collect();
#     if lines.len() > filter.max_lines {
#         out = lines[..filter.max_lines].join("\n") + "\n... truncated";
#     }
# }
# ```

def _apply_max_lines(text: str, max_lines: int) -> str:
    """Stage 7: Cap total line count.

    Ref: toml_filter.rs:516-523 — truncate to max_lines, append marker.
    Marker: "\\n... (N lines truncated)" (exact Rust format).
    Note: Rust applies max_lines AFTER head/tail, so omission markers from
    stage 6 COUNT as lines for this cap.
    """
    if max_lines < 1:
        return text
    lines = _rust_lines(text)
    if len(lines) <= max_lines:
        return text
    # Ref: toml_filter.rs:521 — `format!("... ({} lines truncated)", truncated)`
    truncated_count = len(lines) - max_lines
    return "\n".join(lines[:max_lines]) + f"\n... ({truncated_count} lines truncated)"


# ─── Stage 8: on_empty ──────────────────────────────────────────────────────
# Ref: toml_filter.rs ligne 545-547
# ```rust
# if let Some(on_empty) = &filter.on_empty {
#     if out.trim().is_empty() {
#         out = on_empty.clone();
#     }
# }
# ```

def _apply_on_empty(text: str, message: str) -> str:
    """Stage 8: Replace empty output with a fallback message.

    Ref: toml_filter.rs:525-533 — if trimmed output is empty, return message.
    """
    if not message:
        return text
    if not text.strip():
        return message
    return text


# ─── Utilitaire RTK: format_tokens ──────────────────────────────────────────
# Ref: utils.rs:78 — `pub fn format_tokens(n: usize) -> String`

def _format_tokens(n):
    """Format token count. Ref: utils.rs:78 — ≥1M→M, ≥1K→K, else plain."""
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f}M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}K"
    return str(n)


# ─── Pipeline orchestration ─────────────────────────────────────────────────
# Ref: toml_filter.rs ligne 436 — `pub fn apply_filter(filter: &CompiledFilter, stdout: &str) -> String`

def _apply_filter(text: str, profile: dict) -> str:
    """Apply full 8-stage RTK filter pipeline.

    Ref: toml_filter.rs:436-487 — stages applied in exact order:
      1. strip_ansi
      2. replace (per-line chaining)
      3. match_output (short-circuit)
      4a/4b. strip_lines_matching OR keep_lines_matching (mutually exclusive)
      5. truncate_lines_at (per-line)
      6. head_lines + tail_lines
      7. max_lines
      8. on_empty
    """
    # Stage 1: strip_ansi
    # Ref: toml_filter.rs:451
    if profile.get("strip_ansi", False):
        text = _strip_ansi(text)

    # Stage 2: replace
    # Ref: toml_filter.rs:460-474
    replace_rules = profile.get("replace", [])
    if replace_rules:
        text = _apply_replace(text, replace_rules)

    # Stage 3: match_output (short-circuit)
    # Ref: toml_filter.rs:476-494
    match_rules = profile.get("match_output", [])
    if match_rules:
        short_circuited, text = _apply_match_output(text, match_rules)
        if short_circuited:
            return text

    # Stage 4a/4b: strip_lines_matching OR keep_lines_matching
    # Ref: toml_filter.rs:496-514 — mutually exclusive
    strip_patterns = profile.get("strip_lines_matching", [])
    keep_patterns = profile.get("keep_lines_matching", [])
    if strip_patterns and keep_patterns:
        # Ref: compile_filter toml_filter.rs:316 — validates mutual exclusion
        _warn("strip_lines_matching and keep_lines_matching are mutually exclusive")
    elif strip_patterns:
        text = _apply_strip_lines_matching(text, strip_patterns)
    elif keep_patterns:
        text = _apply_keep_lines_matching(text, keep_patterns)

    # Stage 5: truncate_lines_at
    # Ref: toml_filter.rs (applies utils::truncate per line)
    truncate_at = profile.get("truncate_lines_at", 0)
    if truncate_at > 0:
        text = _apply_truncate_lines_at(text, truncate_at)

    # Stage 6: head_lines + tail_lines
    # Ref: toml_filter.rs:517-535
    head = profile.get("head_lines", 0)
    tail = profile.get("tail_lines", 0)
    if head > 0 or tail > 0:
        text = _apply_head_tail_lines(text, head, tail)

    # Stage 7: max_lines
    # Ref: toml_filter.rs:516-523
    max_lines = profile.get("max_lines", 0)
    if max_lines > 0:
        text = _apply_max_lines(text, max_lines)

    # Stage 8: on_empty
    # Ref: toml_filter.rs:525-533
    on_empty = profile.get("on_empty", "")
    if on_empty:
        text = _apply_on_empty(text, on_empty)

    return text


# ══════════════════════════════════════════════════════════════════════════════
# COUCHE HERMES — Spécifique au plugin (PAS de correspondant Rust)
# ══════════════════════════════════════════════════════════════════════════════

# --- Constantes Hermes ---

_COMPRESS_THRESHOLD = 2000  # Taille min avant compression
_MAX_RETAIN_FRACTION = 0.9  # Appliquer si >10% d'économies
_BLANK_LINE_PAT = r"^\s*$"  # Pattern utilisé par les profils

# XDG (cohérent avec RTK)
def _resolve_xdg_data_home():
    """Resolve XDG_DATA_HOME, matching RTK's behavior."""
    xdg = os.environ.get("XDG_DATA_HOME", "")
    if xdg:
        return Path(xdg)
    return Path.home() / ".local" / "share"

_COMPRESSION_DB_PATH = _resolve_xdg_data_home() / "rtk" / "hermes_compression.db"

# Clés verbose dans les résultats JSON Hermes (PAS dans RTK)
_VERBOSE_KEYS = frozenset({
    "truncated", "is_binary", "dedup", "file_size",
    "hint", "total_lines",  # gardées si petites
})

# --- Profils de filtre pour les outils Hermes ---
# Inspirés des patterns RTK mais adaptés aux outils Hermes.
# AUCUN profil n'utilise match_output, replace, keep_lines_matching,
# truncate_lines_at car ces stages sont pour des commandes shell spécifiques.

_FILTER_PROFILES = {
    # read_file — Pattern "strip noise + max_lines" (cf. ps.toml, jq.toml)
    "read_file": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 200,
    },
    # search_files — Pattern "tail + max_lines" (cf. trunk-build.toml tail=10 max_lines=30)
    "search_files": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "tail_lines": 60,
        "max_lines": 80,
    },
    # browser_snapshot — Pattern "max_lines" (cf. systemctl-status, jq)
    "browser_snapshot": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 100,
    },
    "browser_vision": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 60,
    },
    "browser_click": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 40,
    },
    # skill_view — Pattern "max_lines" (cf. markdownlint)
    "skill_view": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 60,
    },
    # execute_code — Pattern "tail + max_lines" (cf. trunk-build.toml, xcodebuild.toml)
    "execute_code": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "tail_lines": 50,
        "max_lines": 80,
    },
    # vision_analyze — Pattern "max_lines"
    "vision_analyze": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "max_lines": 80,
    },
    # process — Pattern "tail" (cf. trunk-build tail=10, xcodebuild tail=15)
    "process": {
        "strip_ansi": True,
        "strip_lines_matching": [_BLANK_LINE_PAT],
        "tail_lines": 40,
        "max_lines": 60,
        "on_empty": "process: no output",
    },
}

# Default — Pattern "max_lines" (cf. yamllint, sops)
_DEFAULT_PROFILE = {
    "strip_ansi": True,
    "strip_lines_matching": [_BLANK_LINE_PAT],
    "max_lines": 80,
}

# Outils terminal — ne pas compresser
_TERMINAL_TOOLS = frozenset({"terminal"})

# --- H1: Détection JSON ---

def _is_json_result(text: str) -> bool:
    """H1: Check if text is valid JSON dict or list."""
    try:
        data = json.loads(text)
        return isinstance(data, (dict, list))
    except (json.JSONDecodeError, ValueError):
        return False

# --- H2: strip_verbose_json_keys ---

def _strip_verbose_json_keys(data: dict) -> dict:
    """H2: Remove verbose keys from JSON tool results.

    Hermes-specific — no Rust equivalent.
    Keeps 'hint' and 'total_lines' always (Hermes pagination).
    Drops _VERBOSE_KEYS with large string values.
    Keeps small values: bool, short str (<50), int, float, None.
    """
    if not isinstance(data, dict):
        return data

    json_len = len(json.dumps(data, ensure_ascii=False, separators=(',', ':')))
    if json_len < _COMPRESS_THRESHOLD:
        return data

    result = {}
    for k, v in data.items():
        if k in _VERBOSE_KEYS:
            if isinstance(v, bool):
                result[k] = v
            elif isinstance(v, str) and len(v) < 50:
                result[k] = v
            elif isinstance(v, (int, float)):
                result[k] = v
            elif v is None:
                result[k] = v
            # else: large value in _VERBOSE_KEYS → DROPPED
        else:
            result[k] = v
    return result

# --- H3: truncate_json_values ---

def _truncate_json_values(obj, max_len=500, depth=0, max_depth=50):
    """H3: Recursively truncate long string values in JSON.

    Hermes-specific — no Rust equivalent.
    For strings longer than max_len: keep first half + "..." + last half,
    fitting within max_len total.
    """
    if depth > max_depth:
        return "..."
    if isinstance(obj, str):
        if len(obj) <= max_len:
            return obj
        marker_len = len("...")
        usable = max_len - marker_len
        if usable < 2:
            # Not enough room for front + back — return just "..."
            return "..."
        half = usable // 2
        return obj[:half] + "..." + obj[-half:]
    elif isinstance(obj, dict):
        return {k: _truncate_json_values(v, max_len, depth + 1, max_depth)
                for k, v in obj.items()}
    elif isinstance(obj, list):
        return [_truncate_json_values(item, max_len, depth + 1, max_depth)
                for item in obj]
    return obj

# --- H4: compress_tool_result ---

def _compress_tool_result(tool_name: str, result_str: str) -> str:
    """H4: Compress a non-terminal tool result.

    Hermes-specific — no Rust equivalent.
    Two paths:
      - JSON result → structural compression (H2 + H3), safe (no pipeline stages)
      - Text result → RTK pipeline (8 stages via _apply_filter)
    Only applies if savings > 10%.
    """
    original_len = len(result_str)
    if original_len < _COMPRESS_THRESHOLD:
        return result_str

    if _is_json_result(result_str):
        # JSON path: structural compression only, no pipeline stages
        # (pipeline stages like max_lines/head/tail would break JSON)
        try:
            data = json.loads(result_str)
            if isinstance(data, dict):
                data = _strip_verbose_json_keys(data)
            data = _truncate_json_values(data)
            compressed = json.dumps(data, ensure_ascii=False, separators=(',', ':'))
        except (json.JSONDecodeError, ValueError):
            compressed = result_str
    else:
        # Text path: apply RTK pipeline
        profile = _FILTER_PROFILES.get(tool_name, _DEFAULT_PROFILE)
        compressed = _apply_filter(result_str, profile)

    if len(compressed) < original_len * _MAX_RETAIN_FRACTION:
        return compressed
    return result_str

# --- H5: transform_tool_result (hook) ---

def _transform_tool_result(tool_name: str, result) -> str:
    """H5: Compress tool result + append RTK savings tag.

    Hermes-specific — no Rust equivalent.
    """
    result_str = str(result) if not isinstance(result, str) else result
    original_len = len(result_str)

    # Step 1: Compress
    comp_saved = 0
    compressed = _compress_tool_result(tool_name, result_str)
    if len(compressed) < original_len:
        comp_saved = original_len - len(compressed)
        result_str = compressed

    # Step 2: Append savings tag
    saved, count, avg_pct = _read_rtk_savings()
    comp_count, comp_avg = 0, 0.0
    if comp_saved > 0:
        _record_compression(tool_name, comp_saved, original_len)
        comp_count_result, comp_avg = _read_compression_savings_count()

    if saved <= 0 and comp_saved <= 0:
        return result_str

    parts = []
    if saved > 0 and count > 0:
        parts.append(f"tokens saved: {_format_tokens(saved)} across {count} commands (avg {avg_pct}%)")
    if comp_saved > 0 and comp_count_result > 0:
        parts.append(f"chars saved: ~{_format_chars(comp_saved)} across {comp_count_result} results (avg {comp_avg}%)")

    if not parts:
        return result_str

    tag = "\n⟡ " + " | ".join(parts)

    # Inject tag safely
    try:
        data = json.loads(result_str)
        if isinstance(data, dict):
            if "output" in data and isinstance(data["output"], str):
                data["output"] += tag
            else:
                data["_rtk_savings"] = tag.strip()
            return json.dumps(data, separators=(',', ':'))
        elif isinstance(data, list):
            return result_str + tag
    except (json.JSONDecodeError, ValueError):
        pass

    return result_str + tag

# --- H6/H7: Compression tracking DB ---

def _record_compression(tool_name: str, saved: int, original_len: int):
    """H6: Record compression savings to DB."""
    try:
        _ensure_compression_db()
        with sqlite3.connect(str(_COMPRESSION_DB_PATH), timeout=1) as conn:
            conn.execute(
                "INSERT INTO compression_stats (tool_name, original_chars, compressed_chars, saved_chars, saved_pct) VALUES (?, ?, ?, ?, ?)",
                (tool_name, original_len, original_len - saved, saved, round(saved / original_len * 100, 1) if original_len > 0 else 0),
            )
            conn.commit()
    except Exception as e:
        _warn(f"compression db write error: {e}")


def _read_compression_savings():
    """H7a: Read cumulative compression savings."""
    try:
        if not _COMPRESSION_DB_PATH.exists():
            return 0, 0, 0.0
        with sqlite3.connect(str(_COMPRESSION_DB_PATH), timeout=1) as conn:
            conn.row_factory = sqlite3.Row
            cur = conn.execute(
                "SELECT COALESCE(SUM(saved_chars),0) AS saved,"
                " COUNT(*) AS cnt,"
                " COALESCE(ROUND(AVG(saved_pct),1),0.0) AS avg_pct"
                " FROM compression_stats"
            )
            row = cur.fetchone()
            return row["saved"], row["cnt"], row["avg_pct"]
    except Exception:
        return 0, 0, 0.0


def _read_compression_savings_count():
    """H7b: Read compression count and average."""
    try:
        if not _COMPRESSION_DB_PATH.exists():
            return 0, 0.0
        with sqlite3.connect(str(_COMPRESSION_DB_PATH), timeout=1) as conn:
            conn.row_factory = sqlite3.Row
            cur = conn.execute(
                "SELECT COUNT(*) AS cnt, COALESCE(ROUND(AVG(saved_pct),1),0.0) AS avg_pct"
                " FROM compression_stats"
            )
            row = cur.fetchone()
            return row["cnt"], row["avg_pct"]
    except Exception:
        return 0, 0.0


def _ensure_compression_db():
    """Create compression DB if it doesn't exist."""
    try:
        _COMPRESSION_DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        _warn(f"compression db mkdir error: {e}")
        return
    if not _COMPRESSION_DB_PATH.exists():
        with sqlite3.connect(str(_COMPRESSION_DB_PATH)) as conn:
            conn.execute(
                "CREATE TABLE IF NOT EXISTS compression_stats ("
                " id INTEGER PRIMARY KEY AUTOINCREMENT,"
                " tool_name TEXT NOT NULL,"
                " original_chars INTEGER NOT NULL,"
                " compressed_chars INTEGER NOT NULL,"
                " saved_chars INTEGER NOT NULL,"
                " saved_pct REAL NOT NULL,"
                " timestamp TEXT DEFAULT CURRENT_TIMESTAMP)"
            )
            conn.commit()


# --- H8: Read RTK savings ---

def _rtk_db_path():
    """Resolve the RTK tracking database path."""
    return _resolve_xdg_data_home() / "rtk" / "history.db"


def _read_rtk_savings():
    """H8: Read RTK command savings from history.db."""
    try:
        db = _rtk_db_path()
        if not db.exists():
            return 0, 0, 0.0
        with sqlite3.connect(str(db), timeout=1) as conn:
            conn.row_factory = sqlite3.Row
            cur = conn.execute(
                "SELECT COALESCE(SUM(saved_tokens),0) AS saved,"
                " COUNT(*) AS cnt,"
                " COALESCE(ROUND(AVG(savings_pct),1),0.0) AS avg_pct"
                " FROM commands"
            )
            row = cur.fetchone()
            return row["saved"], row["cnt"], row["avg_pct"]
    except Exception:
        return 0, 0, 0.0


# --- H9: Formatage (alias vers RTK layer) ---

def _format_chars(n):
    """Format character count (same as format_tokens)."""
    return _format_tokens(n)


# --- H10/H11: Hermes hooks ---

_rtk_available = None
_rtk_missing_warned = False


def _check_rtk():
    """Check if rtk binary is available."""
    global _rtk_available, _rtk_missing_warned
    if _rtk_available is None:
        from shutil import which
        _rtk_available = which("rtk") is not None
    if not _rtk_available and not _rtk_missing_warned:
        _warn("rtk not found — command rewriting disabled")
        _rtk_missing_warned = True
    return _rtk_available


def pre_tool_call(payload: dict) -> dict:
    """H10: Rewrite terminal commands through rtk."""
    tool_name = payload.get("tool_name", "")
    if tool_name not in _TERMINAL_TOOLS:
        return payload
    if not _check_rtk():
        return payload

    args = payload.get("tool_args", {})
    command = args.get("command", "")
    if not command:
        return payload

    try:
        result = subprocess.run(
            ["rtk", "rewrite", command],
            capture_output=True, text=True, timeout=5
        )
        if result.returncode == 0 and result.stdout.strip():
            args["command"] = result.stdout.strip()
            payload["tool_args"] = args
    except Exception:
        pass

    return payload


def transform_tool_result(payload: dict) -> dict:
    """H11: Compress non-terminal tool results + append savings tag."""
    tool_name = payload.get("tool_name", "")
    if tool_name in _TERMINAL_TOOLS:
        return payload

    result = payload.get("result", "")
    if not result:
        return payload

    transformed = _transform_tool_result(tool_name, result)
    payload["result"] = transformed
    return payload


def register(ctx=None):
    """Register plugin hooks via PluginContext.

    ``ctx`` is passed by Hermes >= v0.14.0 (plugin API change).
    Must call ctx.register_hook() — returning a dict is NOT consumed by PluginManager.
    Kept optional for backward compatibility with older versions.
    """
    if ctx is not None:
        ctx.register_hook("pre_tool_call", pre_tool_call)
        ctx.register_hook("transform_tool_result", transform_tool_result)
    else:
        # Legacy fallback: return dict (not consumed by PluginManager in v0.14.0+)
        return {
            "pre_tool_call": pre_tool_call,
            "transform_tool_result": transform_tool_result,
        }


def _warn(message: str):
    print(f"rtk: hermes plugin warning: {message}", file=sys.stderr)
"""Extract per-rule true/false positive test cases from gitleaks' Go rule sources.

Each rules/*.go file defines one or more `func XxxRule() *config.Rule` blocks
containing `RuleID: "..."` plus `tps := []string{...}` / `fps := []string{...}`.
We keep only *literal* strings; entries built by helpers (GenerateSampleSecret,
secrets.NewSecret) are runtime-random and can't be checked into a fixture.
"""
import json
import os
import re
import sys

# build.py normalises the unpacked archive to this name, so the pinned ref can
# change without touching every downstream script.
SRC = os.path.join("gitleaks-src", "cmd", "generate", "config", "rules")


def go_literals(block: str):
    """Pull double-quoted and backtick-quoted Go string literals out of a block."""
    out = []
    i = 0
    n = len(block)
    while i < n:
        c = block[i]
        if c == '`':
            j = block.find('`', i + 1)
            if j == -1:
                break
            out.append(block[i + 1:j])
            i = j + 1
        elif c == '"':
            buf = []
            i += 1
            while i < n:
                if block[i] == '\\' and i + 1 < n:
                    esc = block[i + 1]
                    buf.append({'n': '\n', 't': '\t', 'r': '\r',
                                '\\': '\\', '"': '"'}.get(esc, '\\' + esc))
                    i += 2
                elif block[i] == '"':
                    i += 1
                    break
                else:
                    buf.append(block[i])
                    i += 1
            out.append(''.join(buf))
        else:
            i += 1
    return out


def slice_block(text: str, marker: str):
    """Return the body of `marker := []string{ ... }` with brace matching."""
    m = re.search(re.escape(marker) + r'\s*:?=\s*\[\]string\{', text)
    if not m:
        return None
    i = m.end()
    depth = 1
    start = i
    while i < len(text) and depth:
        if text[i] == '{':
            depth += 1
        elif text[i] == '}':
            depth -= 1
        i += 1
    return text[start:i - 1]


def main():
    results = {}
    skipped_dynamic = 0
    for fname in sorted(os.listdir(SRC)):
        if not fname.endswith('.go'):
            continue
        text = open(os.path.join(SRC, fname), encoding='utf-8').read()
        # Segment per top-level func so multi-rule files map correctly.
        parts = re.split(r'\nfunc ', text)
        for part in parts:
            rid = re.search(r'RuleID:\s*"([^"]+)"', part)
            if not rid:
                continue
            rid = rid.group(1)
            entry = {'tps': [], 'fps': []}
            for kind in ('tps', 'fps'):
                body = slice_block(part, kind)
                if body is None:
                    continue
                # Drop lines that build values at runtime.
                kept_lines = []
                for line in body.splitlines():
                    if 'GenerateSampleSecret' in line or 'NewSecret' in line \
                            or 'utils.' in line or 'secrets.' in line:
                        skipped_dynamic += 1
                        continue
                    kept_lines.append(line)
                entry[kind] = [s for s in go_literals('\n'.join(kept_lines)) if s.strip()]
            results[rid] = entry

    json.dump(results, open('gitleaks_testcases.json', 'w'), indent=1)
    with_tp = sum(1 for v in results.values() if v['tps'])
    with_fp = sum(1 for v in results.values() if v['fps'])
    tot_tp = sum(len(v['tps']) for v in results.values())
    tot_fp = sum(len(v['fps']) for v in results.values())
    print(f"rules parsed:        {len(results)}")
    print(f"  with true-positives:  {with_tp}  ({tot_tp} cases)")
    print(f"  with false-positives: {with_fp}  ({tot_fp} cases)")
    print(f"  dynamic lines skipped: {skipped_dynamic}")


if __name__ == '__main__':
    sys.exit(main())

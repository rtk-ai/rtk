#!/usr/bin/env python3
import json, os, re, subprocess, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pipeline as pl

RUN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "run.sh")

FILTERS = [
    (r"git\s+status\b", "git-status", None),
    (r"(grep|egrep)\b", "grep", None),
    (r"rg\b", "rg", None),
    (r"find\b", "find", None),
    (r"fd\b", "fd", None),
    (r"(python3?\s+-m\s+)?pytest\b", "pytest", r"\d+ (passed|failed|errors?|skipped|xfailed|xpassed|deselected)\b|^(FAILED|ERROR) |^E  "),
    (r"cargo\s+test\b", "cargo-test", r"^test result:|^running \d+ tests?$|^test \S+ \.\.\. "),
    (r"go\s+test\b", "go-test", r"^(ok|FAIL|PASS)\b|^--- (PASS|FAIL|SKIP)|^=== RUN|no test files"),
    (r"go\s+build\b", "go-build", r"^# |\.go:\d+:"),
    (r"(npx\s+)?tsc\b", "tsc", r"error TS\d+"),
    (r"(npx\s+)?vitest\b", "vitest", r"Test Files|Tests\s+\d+|[✓✗×]|\b(FAIL|PASS)\b"),
    (r"(python3?\s+-m\s+)?mypy\b", "mypy", r"^Success:|: error:|^Found \d+ errors?"),
    (r"ruff\s+check\b", "ruff-check", r"^All checks passed|^Found \d+ errors?|^\S+:\d+:\d+: [A-Z]+\d+"),
]
LISTING = re.compile(r"(^|\s)(-h|--help|--version|--collect-only|--co|--fixtures|--markers|--list|-list|--listFiles|--listFilesOnly|--showConfig|--explainFiles|--show-files|--show-settings|--reporter[= ]json|--json)(=|\s|$)")
MIN_BYTES = 1024
MAX_RATIO = 0.8


def pick_filter(command):
    cmd = command.strip()
    cmd = re.sub(r'^cd\s+("[^"]*"|\'[^\']*\'|\S+)\s*(&&|;)\s*', "", cmd)
    if re.search(r"&&|\|\||;|\||>|<|`|\$\(", cmd):
        return None, None
    for pat, name, marker in FILTERS:
        if re.match(pat, cmd):
            return (None, None) if LISTING.search(cmd) else (name, marker)
    return None, None


def looks_like(marker, text):
    return not marker or re.search(marker, text, re.M) is not None


TRUNC = re.compile(r"\n?\[output truncated[^\]]*\]\s*$")

SAVED = re.compile(r"Output has been saved to (/\S+?\.(?:txt|json))")


def load_saved(path):
    root = os.path.realpath(os.path.expanduser("~/.claude"))
    real = os.path.realpath(path)
    if not real.startswith(root + os.sep) or "/tool-results/" not in real:
        return None
    try:
        with open(real, errors="replace") as fh:
            data = fh.read()
    except Exception:
        return None
    try:
        blocks = json.loads(data)
        if isinstance(blocks, list):
            return blocks
    except Exception:
        pass
    return [{"type": "text", "text": data}]


def kb(n):
    return f"{n / 1024:.0f} KB" if n >= 1024 else f"{n} B"


def log(ev, status, name="", raw=0, filtered=0, command=""):
    try:
        with open(pl.DEVICE_LOG, "a") as fh:
            fh.write(f"{ev.get('session_id', '')}\t{time.strftime('%H:%M:%S')}\t{status}\t{name}\t{raw}\t{filtered}\t{' '.join(command.split())[:120]}\n")
    except Exception:
        pass


def pipeline_rules(ev, command, text):
    if pl.is_off():
        log(ev, "skip-nofilter", "", len(text), 0, command)
        return
    fed = dict(ev, tool_name="Bash", tool_input={"command": pl.norm(command)}, tool_response={"stdout": text, "stderr": "", "interrupted": False})
    try:
        tp = ev.get("transcript_path")
        if tp and os.path.isfile(tp):
            fed["transcript_path"] = pl.present(tp, ev.get("session_id") or "x", None)[0]
        r = subprocess.run(["sh", RUN], input=json.dumps(fed), capture_output=True, text=True, timeout=80, env=pl.rtk_env())
        new = (json.loads(r.stdout).get("hookSpecificOutput") or {}).get("updatedToolOutput") if r.stdout.strip() else None
    except Exception:
        log(ev, "skip-error", "", len(text), 0, command)
        return
    if isinstance(new, dict):
        new = new.get("stdout")
    if not isinstance(new, str) or not new.strip() or len(new) >= len(text):
        log(ev, "skip-nofilter", "", len(text), 0, command)
        return
    log(ev, "ok-pipeline", "", len(text), len(new), command)
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PostToolUse", "updatedMCPToolOutput": [{"type": "text", "text": new}]}}))


def main():
    try:
        ev = json.load(sys.stdin)
    except Exception:
        return
    if not isinstance(ev, dict):
        return
    command = pl.command_of(ev)
    resp = ev.get("tool_response")
    if isinstance(resp, dict):
        resp = resp.get("content", [resp])
    if isinstance(resp, str):
        m = SAVED.search(resp)
        if m:
            resp = load_saved(m.group(1))
            if resp is None:
                log(ev, "skip-saved", command=command)
                return
        else:
            resp = [{"type": "text", "text": resp}]
    if not isinstance(resp, list) or not resp or not all(isinstance(b, dict) and b.get("type") == "text" for b in resp):
        log(ev, "skip-shape", command=command)
        return
    text = "\n".join(b.get("text") or "" for b in resp)
    note = ""
    m = TRUNC.search(text)
    if m:
        note, text = m.group(0).strip(), text[:m.start()]
    name, marker = pick_filter(pl.norm(command))
    if len(text) < MIN_BYTES:
        log(ev, "skip-small", name or "", len(text), 0, command)
        return
    if not name:
        pipeline_rules(ev, command, text)
        return
    if not looks_like(marker, text):
        log(ev, "skip-marker", name, len(text), 0, command)

        pipeline_rules(ev, command, text)
        return
    try:
        out = subprocess.run([pl.rtk(), "pipe", "-f", name], input=text, capture_output=True, text=True, timeout=20)
    except Exception:
        log(ev, "skip-error", name, len(text), 0, command)
        return
    filtered = out.stdout
    if out.returncode != 0 or not filtered.strip() or len(filtered) > MAX_RATIO * len(text):
        log(ev, "skip-ratio", name, len(text), len(filtered), command)
        pipeline_rules(ev, command, text)
        return
    if text.count("\n") >= 20 and len(filtered.strip().splitlines()) <= 1 and not re.search(r"\d", filtered):
        log(ev, "skip-floor", name, len(text), len(filtered), command)
        pipeline_rules(ev, command, text)
        return

    tag = f"[rtk: {kb(len(text))} -> {kb(len(filtered))} (-{100 - 100 * len(filtered) // len(text)}%)"
    if note:
        tag += f"; output was cut off at {kb(len(text))} before compression, counts cover only that part"
    filtered = tag + "]\n" + filtered
    log(ev, "ok", name, len(text), len(filtered), command)
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PostToolUse",
        "updatedMCPToolOutput": [{"type": "text", "text": filtered}],
    }}))


if __name__ == "__main__":
    main()

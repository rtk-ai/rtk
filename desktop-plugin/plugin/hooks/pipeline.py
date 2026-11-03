import collections, glob, json, os, re, shutil, subprocess, sys, tempfile, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATE = os.environ.get("RTK_COWORK_STATE") or "/tmp/rtk-cowork"

OFF_CFG = os.path.join(STATE, "cfg-off")
LABELS = os.path.join(STATE, "labels.json")
SETUP = os.path.join(STATE, "setup.json")
DEVICE_LOG = STATE + "-device.log"

DEV = "mcp__remote-devices__device_bash"
DEFAULTS = ("default-bash-compress", "explore-first")
HIDDEN = re.compile(r"^(ToolSearch$|mcp__(remote-devices__)?plugin_rtk(-cowork)?_|mcp__remote-devices__get_device_info$)")
LIST_DIR = "mcp__remote-devices__device_list_dir"
WRITES = re.compile(r"<<|(^|[^2&])>(?!\s*/dev/null)|\b(sed -i|perl -pi|tee|cp|mv|rm|mkdir|touch|patch|git (add|commit|apply|checkout|restore))\b")

PRICES = {
    "opus-5-5": (4.0, 5.0, 8.0, 0.20, 20.0),
    "fable-5-1": (10.0, 12.5, 20.0, 0.25, 50.0),
    "sonnet-5-5": (2.0, 2.5, 4.0, 0.20, 10.0),
    "haiku-5-5": (0.10, 0.125, 0.20, 0.01, 0.50),
}


def rtk():
    r = os.path.join(ROOT, "runtime", "rtk")
    if os.access(r, os.X_OK):
        return r
    c = os.path.expanduser("~/.cache/rtk-cowork/rtk")
    if not os.access(c, os.X_OK):
        os.makedirs(os.path.dirname(c), exist_ok=True)
        shutil.copy(r, c)
        os.chmod(c, 0o755)
    return c


def cowork():
    return sys.platform.startswith("linux") and os.path.isdir("/home/claude")


def safe(s):
    return re.sub(r"[^\w-]", "_", str(s or "x"))[:80]


def note(name, what):
    try:
        os.makedirs(STATE, exist_ok=True)
        with open(os.path.join(STATE, name), "a") as fh:
            fh.write(f"{time.strftime('%H:%M:%S')} {what}\n")
    except OSError:
        pass


def is_off():
    try:
        return open(os.path.join(STATE, "pipeline")).read().strip() == "off"
    except OSError:
        return False


def read_json(path, default):
    try:
        with open(path) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return default


def pipelines_dir():
    cfg = os.environ.get("RTK_CONFIG_DIR")
    if cfg:
        return os.path.join(cfg, "pipelines")
    out = subprocess.run([rtk(), "config"], capture_output=True, text=True).stdout
    for line in out.splitlines():
        if line.startswith("Config: "):
            return os.path.join(os.path.dirname(line[8:].strip()), "pipelines")
    return os.path.expanduser("~/.config/rtk/pipelines")


def add(path):
    return subprocess.run([rtk(), "pipeline", "add", path, "--force"], stdin=subprocess.DEVNULL, capture_output=True, text=True)


def ensure_defaults():
    os.makedirs(STATE, exist_ok=True)
    mark = os.path.join(STATE, "installed")
    if os.path.exists(mark):
        return
    for name in DEFAULTS:
        add(os.path.join(ROOT, "pipeline", name + ".toml"))
    subprocess.run([rtk(), "pipeline", "log", "on"], capture_output=True)
    with open(mark, "w") as fh:
        fh.write(time.strftime("%H:%M:%S\n"))


def apply_setup(data):
    ensure_defaults()
    failed = []
    active = data.get("active") or list(DEFAULTS)
    pdir = pipelines_dir()
    for f in glob.glob(os.path.join(pdir, "*.toml")):
        if os.path.basename(f)[:-5] not in active:
            os.remove(f)
    for name, toml in (data.get("pipelines") or {}).items():
        path = os.path.join(STATE, safe(name) + ".toml")
        with open(path, "w") as fh:
            fh.write(str(toml))
        r = add(path)
        if r.returncode != 0 or not os.path.exists(os.path.join(pdir, name + ".toml")):
            failed.append(name)
    if data.get("fallback"):
        add(os.path.join(ROOT, "pipeline", "explore-first.toml"))
    labels = data.get("labels") or {}
    if labels:
        with open(LABELS, "w") as fh:
            json.dump(labels, fh)
        subprocess.Popen([sys.executable, os.path.join(ROOT, "hooks", "laya_bridge.py")], stdin=subprocess.DEVNULL,
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)
    setup = read_json(SETUP, {})
    setup.update(done=True, active=active, failed=failed, os=data.get("os") or "")
    with open(SETUP, "w") as fh:
        json.dump(setup, fh)
    return failed


def n_of(v):
    return int(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else 0


def norm(cmd):
    return re.sub(r"^(\s*cd [^\n;&|]+?)[ \t]*\n\s*", r"\1 && ", cmd if isinstance(cmd, str) else "")


def tool_input(ev):
    ti = ev.get("tool_input") if isinstance(ev, dict) else None
    return ti if isinstance(ti, dict) else {}


def command_of(ev):
    c = tool_input(ev).get("command")
    return c if isinstance(c, str) else ""



def history_command(cmd):
    cmd = re.sub(r"\|(\s*)([A-Z])", lambda m: "|" + m.group(1) + m.group(2).lower(), cmd.replace("\\|", "/"))
    head = cmd[:115].rstrip()
    return head[:-1].rstrip() if len(cmd) > 115 and head.endswith("|") else cmd


def present(src, sid, skip_id):
    dst = os.path.join(STATE, f"transcript-{safe(sid)}.jsonl")
    os.makedirs(STATE, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=f"transcript-{safe(sid)}.", suffix=".tmp", dir=STATE)
    asked_setup = False
    goal, seen_calls = "", []
    hide = set(read_json(SETUP, {}).get("hide") or [])
    with open(src, "rb") as f, open(fd, "w") as g:

        for line in f:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if not isinstance(e, dict) or e.get("type") not in ("user", "assistant") or not isinstance(e.get("message"), dict):
                continue
            c = e["message"].get("content")
            if e.get("type") == "user" and not e.get("isMeta"):
                for text in [c] if isinstance(c, str) else [b.get("text") for b in c if isinstance(b, dict) and b.get("type") == "text"] if isinstance(c, list) else []:
                    if isinstance(text, str) and text.strip() and not text.lstrip().startswith("<"):
                        goal = text
            if isinstance(c, list):
                kept = []
                for b in c:
                    if isinstance(b, dict) and b.get("type") == "tool_use":
                        n = str(b.get("name") or "")
                        if n.endswith("__rtk_task_setup"):
                            asked_setup = True
                        if b.get("id") == skip_id or b.get("id") in hide or HIDDEN.match(n):
                            continue
                        inp = b.get("input") if isinstance(b.get("input"), dict) else {}
                        if n == DEV:
                            cmd = norm(inp.get("command"))
                            b = dict(b, name="Edit", input={"file_path": "file"}) if WRITES.search(cmd) else dict(b, name="Bash", input={"command": history_command(cmd)})
                        elif n == "Bash":
                            b = dict(b, input=dict(inp, command=history_command(str(inp.get("command") or ""))))
                        elif n == LIST_DIR:
                            b = dict(b, name="Bash", input={"command": "ls"})
                        i = b.get("input") if isinstance(b.get("input"), dict) else {}
                        seen_calls.append((b.get("name"), str(i.get("command") or i.get("file_path") or i.get("pattern") or "")))
                    kept.append(b)
                if c and not kept:
                    continue
                e["message"]["content"] = kept
            g.write(json.dumps(e) + "\n")
    os.replace(tmp, dst)
    return dst, asked_setup, goal, seen_calls


def call_state(command, goal, calls):
    recent = " | ".join(f"{n} {c[:120]}".rstrip() for n, c in reversed(calls[-5:]))
    return f"command: {command[:300]}\ngoal: {goal[:600]}\nrecent_calls: {recent}\ntool: shell\n"


def rtk_env():
    env = dict(os.environ, RTK_LAYA_TOKEN=os.environ.get("RTK_LAYA_TOKEN") or "local")
    if is_off():
        env["RTK_CONFIG_DIR"] = OFF_CFG
    return env


def on():
    os.makedirs(STATE, exist_ok=True)
    with open(os.path.join(STATE, "pipeline"), "w") as fh:
        fh.write("on\n")
    ensure_defaults()
    print("RTK pipelines are on for this task.")


def off():
    os.makedirs(STATE, exist_ok=True)
    os.makedirs(OFF_CFG, exist_ok=True)
    with open(os.path.join(STATE, "pipeline"), "w") as fh:
        fh.write("off\n")
    print("RTK pipelines are off for this task (output compression stays on).")


def transcripts():
    return glob.glob(os.path.expanduser("~/.claude/projects/**/*.jsonl"), recursive=True)


def report():
    usage, seen, messages = collections.defaultdict(lambda: [0, 0, 0, 0, 0]), set(), {}
    calls = collections.Counter()
    agents = collections.Counter()
    stamps = []
    for path in transcripts():
        for line in open(path, errors="replace"):
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if not isinstance(e, dict) or e.get("type") not in ("user", "assistant"):
                continue
            if isinstance(e.get("timestamp"), str):
                stamps.append(e["timestamp"])
            m = e.get("message") if isinstance(e.get("message"), dict) else {}
            who = "sub" if e.get("isSidechain") or "/subagents/" in path else "main"
            if e.get("type") == "assistant" and isinstance(m.get("content"), list):
                for b in m["content"]:
                    if isinstance(b, dict) and b.get("type") == "tool_use":
                        key = (m.get("id"), b.get("id"))
                        if key in seen:
                            continue
                        seen.add(key)
                        calls[(who, str(b.get("name") or "?"))] += 1
                        if b.get("name") in ("Agent", "Task"):
                            i = b.get("input") if isinstance(b.get("input"), dict) else {}
                            agents[f"{i.get('subagent_type', '?')} on {i.get('model', 'default')}"] += 1

            u = m.get("usage")
            if e.get("type") == "assistant" and isinstance(u, dict):
                old = messages.get(m.get("id"))
                if not old or n_of(u.get("output_tokens")) >= n_of(old[1].get("output_tokens")):
                    messages[m.get("id")] = (str(m.get("model") or "?"), u)
    for model, u in messages.values():
        cc = u.get("cache_creation") if isinstance(u.get("cache_creation"), dict) else {}
        w1h = cc.get("ephemeral_1h_input_tokens")
        w5m = cc.get("ephemeral_5m_input_tokens")
        if w1h is None and w5m is None:
            w1h, w5m = u.get("cache_creation_input_tokens", 0), 0
        row = usage[model]
        for i, v in enumerate((u.get("input_tokens", 0), w5m, w1h, u.get("cache_read_input_tokens", 0), u.get("output_tokens", 0))):
            row[i] += n_of(v)

    def k(n):
        return f"{n / 1e6:.2f}M" if n >= 1e6 else f"{n / 1e3:.1f}K" if n >= 1e3 else str(n)

    def lines_of(name):
        try:
            with open(os.path.join(STATE, name)) as fh:
                return [l.rstrip("\n") for l in fh if l.strip()]
        except OSError:
            return []

    setup = read_json(SETUP, {})
    active = sorted(os.path.basename(f)[:-5] for f in glob.glob(os.path.join(pipelines_dir(), "*.toml")))
    head = ["pipelines off" if is_off() else "pipelines: " + (", ".join(active) or "none"),
            subprocess.run([rtk(), "--version"], capture_output=True, text=True).stdout.strip()]
    labels = read_json(LABELS, {})
    if labels:
        head.append("Laya labels (on your computer): " + ", ".join(f"{q}={a.get('choice')} ({a.get('confidence', 0):.2f})" for q, a in labels.items()))
    if stamps:
        t0, t1 = min(stamps), max(stamps)
        try:
            d = time.mktime(time.strptime(t1[:19], "%Y-%m-%dT%H:%M:%S")) - time.mktime(time.strptime(t0[:19], "%Y-%m-%dT%H:%M:%S"))
            head.append(f"wall-clock {int(d // 60)}m{int(d % 60):02d}s")
        except (ValueError, OverflowError):
            pass

    out = ["## RTK report", "- " + " · ".join(head)]
    if setup.get("failed"):
        out.append("- ⚠️ could not install: " + ", ".join(setup["failed"]))
    out += ["", "| Model | Input | Write 5m | Write 1h | Read | Output | Est. $ |", "|---|---:|---:|---:|---:|---:|---:|"]
    total = 0.0
    for model, (i, w5, w1, rd, o) in sorted(usage.items()):
        if not any((i, w5, w1, rd, o)):
            continue
        p = next((v for key, v in PRICES.items() if key in model), None)
        cost = sum(a * b for a, b in zip((i, w5, w1, rd, o), p)) / 1e6 if p else 0.0
        total += cost
        out.append(f"| {model} | {k(i)} | {k(w5)} | {k(w1)} | {k(rd)} | {k(o)} | {cost:.3f}{'' if p else ' (no price)'} |")
    out += [f"| **total** | | | | | | **{total:.3f}** |", "", "_API list prices; Claude Code writes cache at the 1h rate._"]
    for who, label in (("main", "calls, main agent"), ("sub", "calls, sub-agents")):
        row = sorted(((n, c) for (w, n), c in calls.items() if w == who), key=lambda x: -x[1])
        if row:
            out.append(f"- {label}: " + ", ".join(f"{n.replace('mcp__remote-devices__', '')} {c}" for n, c in row))
    if agents:
        out.append("- sub-agents: " + ", ".join(f"{a} ×{c}" for a, c in agents.items()))
    pre = collections.Counter()
    for line in lines_of("pre.log"):
        f = line.split("\t")
        if len(f) >= 5:
            pre[(f[2], f[3].replace("mcp__remote-devices__", ""), f[4].split(":")[0])] += 1
    if pre:
        out.append("- hook decisions: " + ", ".join(f"{w}/{t}/{d} {c}" for (w, t, d), c in sorted(pre.items())))
    plugin = collections.Counter()
    for line in lines_of("allow.log"):
        f = line.split("\t")
        if len(f) >= 4:
            plugin[(f[2], f[3].split(",")[0])] += 1
    if plugin:
        out.append("- plugin tools: " + ", ".join(f"{t} {w} {c}" for (t, w), c in sorted(plugin.items())))
    for p in glob.glob(os.path.join(STATE, "transcript-*.jsonl"))[:1]:
        seen_by_rtk = []
        with open(p, errors="replace") as fh:
            for line in fh:
                try:
                    content = (json.loads(line).get("message") or {}).get("content") or []
                except (ValueError, AttributeError):
                    continue
                if not isinstance(content, list):
                    continue
                for b in content:

                    if isinstance(b, dict) and b.get("type") == "tool_use":
                        inp = b.get('input') if isinstance(b.get('input'), dict) else {}
                        seen_by_rtk.append(f"{b.get('name')} {str(inp.get('command') or '')[:48]}".strip())
        out.append("- rtk sees: " + " · ".join(f"`{s}`" for s in seen_by_rtk[:4]))
    log = subprocess.run([rtk(), "pipeline", "log", "show"], capture_output=True, text=True).stdout
    lines = dict.fromkeys(l.rstrip() for l in log.splitlines())
    rules = collections.Counter(l.split()[-2] for l in lines if l.endswith((" deny", " route")))
    if rules:
        out.append("- rtk blocks by rule: " + ", ".join(f"{r} {c}" for r, c in rules.items()))
    n, raw, o1, n2, raw2, o2 = 0, 0, 0, 0, 0, 0
    try:
        with open(DEVICE_LOG) as fh:
            for line in fh:
                f = line.rstrip("\n").split("\t")
                if len(f) >= 6 and f[2] == "ok":
                    n, raw, o1 = n + 1, raw + int(f[4]), o1 + int(f[5])
                elif len(f) >= 6 and f[2] == "ok-pipeline":
                    n2, raw2, o2 = n2 + 1, raw2 + int(f[4]), o2 + int(f[5])
    except OSError:
        pass
    out.append(f"- folder outputs compressed in the workspace: {n} by rtk filters, {k(raw // 4)} → {k(o1 // 4)} tokens; {n2} by pipeline rules, {k(raw2 // 4)} → {k(o2 // 4)} tokens")
    stop = lines_of("stop.log")
    if stop:
        out.append("- stop: " + " · ".join(stop))
    print("\n".join(out))


if __name__ == "__main__":
    if not cowork():
        print("This RTK command works in Claude Desktop tasks only.")
        sys.exit(0)
    {"on": on, "off": off, "report": report}.get(sys.argv[1] if len(sys.argv) > 1 else "report", report)()


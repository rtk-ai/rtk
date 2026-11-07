import collections, glob, json, os, re, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pipeline as pl

START = os.path.join(tempfile.gettempdir(), "rtk-cowork-summary")


def tokens(text):
    return len(text) // 4


def rtk_totals():
    try:
        out = subprocess.run([pl.rtk(), "gain", "--format", "json"], capture_output=True, text=True, timeout=10)
        s = json.loads(out.stdout)["summary"]
        return int(s.get("total_commands", 0)), int(s.get("total_saved", 0))
    except Exception:
        return 0, 0


def start_totals(session):
    try:
        with open(os.path.join(START, pl.safe(session) + ".json")) as fh:
            st = json.load(fh)
        return int(st.get("cmds", 0)), int(st.get("saved", 0))
    except Exception:
        return 0, 0


def device_totals(session):
    n = saved = 0
    try:
        with open(pl.DEVICE_LOG) as fh:
            for line in fh:
                f = line.rstrip("\n").split("\t")
                if len(f) >= 6 and f[0] == session and f[2] in ("ok", "ok-pipeline"):
                    n += 1
                    saved += max(0, int(f[4]) - int(f[5])) // 4
    except Exception:
        pass
    return n, saved


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b["text"] for b in content if isinstance(b, dict) and b.get("type") == "text" and isinstance(b.get("text"), str))
    return ""



def cost_of(model, u):
    p = next((v for key, v in pl.PRICES.items() if key in (model or "")), None)
    if not p:
        return 0.0
    cc = u.get("cache_creation") if isinstance(u.get("cache_creation"), dict) else {}
    w1h, w5m = cc.get("ephemeral_1h_input_tokens"), cc.get("ephemeral_5m_input_tokens")
    if w1h is None and w5m is None:
        w1h, w5m = u.get("cache_creation_input_tokens", 0), 0
    row = (u.get("input_tokens", 0), w5m, w1h, u.get("cache_read_input_tokens", 0), u.get("output_tokens", 0))
    return sum(pl.n_of(a) * b for a, b in zip(row, p)) / 1e6


def read_transcript(path):
    msgs, agents, read, last_text, sub, first, dev, out = {}, [], 0, "", "/subagents/" in path, "", 0, collections.Counter()
    with open(path, errors="replace") as fh:
        entries = fh.readlines()
    for line in entries:
        try:
            e = json.loads(line)
        except ValueError:
            continue
        if not isinstance(e, dict) or e.get("type") not in ("user", "assistant"):
            continue
        if e.get("isSidechain"):
            sub = True
        if isinstance(e.get("timestamp"), str) and (not first or e["timestamp"] < first):
            first = e["timestamp"]
        m = e.get("message") if isinstance(e.get("message"), dict) else {}
        c = m.get("content")
        if e.get("type") == "user" and isinstance(c, list):
            for b in c:
                if isinstance(b, dict) and b.get("type") == "tool_result":
                    read += tokens(text_of(b.get("content")))
        if e.get("type") == "assistant":
            if isinstance(c, list):
                for b in c:
                    if isinstance(b, dict) and b.get("type") == "tool_use":
                        if b.get("name") in ("Agent", "Task"):
                            i = b.get("input") if isinstance(b.get("input"), dict) else {}
                            agents.append((str(i.get("subagent_type") or "helper"), str(i.get("model") or "default")))

                        if b.get("name") == pl.DEV:
                            dev += 1
            t = text_of(c)
            if t.strip():
                last_text = t
            u = m.get("usage")
            if isinstance(u, dict):
                old = msgs.get(m.get("id"))
                if not old or pl.n_of(u.get("output_tokens")) >= pl.n_of(old[1].get("output_tokens")):
                    msgs[m.get("id")] = (str(m.get("model") or "?"), u)
    for model, u in msgs.values():
        out[model] += pl.n_of(u.get("output_tokens"))
    cost = collections.defaultdict(float)
    for model, u in msgs.values():
        cost[model] += cost_of(model, u)
    return {"path": path, "sub": sub, "first": first, "cost": dict(cost), "output": dict(out), "read": read,
            "report": tokens(last_text), "agents": agents, "dev": dev}


def rewrites(session):
    n = 0
    try:
        with open(os.path.join(pl.STATE, "pre.log")) as fh:
            for line in fh:
                f = line.rstrip("\n").split("\t")
                if len(f) >= 5 and f[0] == session and f[4] == "rewrite":
                    n += 1
    except OSError:
        pass
    return n


def rules_fired():
    try:
        log = subprocess.run([pl.rtk(), "pipeline", "log", "show"], capture_output=True, text=True, timeout=10).stdout
    except Exception:
        return {}
    lines = dict.fromkeys(l.rstrip() for l in log.splitlines())
    return dict(collections.Counter(l.split()[-2] for l in lines if l.endswith((" deny", " route"))))


def build(session):
    session = str(session or "x")
    files = [read_transcript(p) for p in pl.transcripts()]

    mains, subs = [f for f in files if not f["sub"]], sorted((f for f in files if f["sub"]), key=lambda f: f["first"])
    agents = [a for f in mains for a in f["agents"]]
    by_model = collections.defaultdict(float)
    for f in files:
        for model, c in f["cost"].items():
            by_model[model] += c
    main_cost = sum(c for f in mains for c in f["cost"].values())
    main_model = max((m for f in mains for m in f["output"]), key=lambda m: sum(f["output"].get(m, 0) for f in mains), default="?")
    helpers = []
    for i, f in enumerate(subs):
        kind, _ = agents[i] if i < len(agents) else ("helper", "default")
        model = max(f["output"], key=f["output"].get, default="?")
        wrote = sum(f["output"].values())
        helpers.append({"type": kind, "model": short(model), "read": f["read"], "wrote": wrote, "report": f["report"],
                        "kept": max(0, f["read"] + wrote - f["report"]), "usd": round(sum(f["cost"].values()), 4)})
    cmds, saved = rtk_totals()
    c0, s0 = start_totals(session)
    dn, ds = device_totals(session)
    setup = pl.read_json(pl.SETUP, {})
    rec = {
        "v": 1, "sid": session[:80], "at": time.strftime("%Y-%m-%d"), "os": setup.get("os") or "", "model": short(main_model),
        "folder": any(f["dev"] for f in mains), "off": pl.is_off(),
        "pipelines": sorted(os.path.basename(p)[:-5] for p in glob.glob(os.path.join(pl.pipelines_dir(), "*.toml"))) if not pl.is_off() else [],
        "compression": {"commands": rewrites(session), "saved": max(0, saved - s0)},
        "device": {"outputs": dn, "saved": ds},
        "delegation": {"kept": sum(h["kept"] for h in helpers), "helpers": helpers},
        "cost": {"main_usd": round(main_cost, 4), "helpers_usd": round(sum(h["usd"] for h in helpers), 4), "by_model": {short(m): round(c, 4) for m, c in by_model.items()}},
        "rules": rules_fired(),
        "laya": {q: a.get("choice") for q, a in pl.read_json(pl.LABELS, {}).items()},
    }
    return rec


def short(model):
    m = re.search(r"(opus|sonnet|haiku|fable)-(\d+)-(\d+)", model or "")
    return f"{m.group(1)}-{m.group(2)}.{m.group(3)}" if m else (model or "?")


def gained(rec):
    return rec["compression"]["saved"] + rec["device"]["saved"] + rec["delegation"]["kept"]


if __name__ == "__main__":
    ev = json.load(sys.stdin) if not sys.stdin.isatty() else {}
    print(json.dumps(build(ev.get("session_id") or (sys.argv[1] if len(sys.argv) > 1 else "x"))))


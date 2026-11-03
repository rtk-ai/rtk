import json, os, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ledger
import pipeline as pl


def fmt(n):
    return f"{n / 1e6:.1f}M" if n >= 1e6 else f"{n / 1e3:.1f}K" if n >= 1e3 else str(n)


def note(what):
    try:
        os.makedirs(pl.STATE, exist_ok=True)
        with open(os.path.join(pl.STATE, "stop.log"), "a") as fh:
            fh.write(f"{time.strftime('%H:%M:%S')} {what}\n")
    except OSError:
        pass


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    try:
        ev = json.load(sys.stdin)
    except Exception:
        return
    session = ev.get("session_id") or "x"
    os.makedirs(ledger.START, exist_ok=True)
    path = os.path.join(ledger.START, pl.safe(session) + ".json")
    if mode == "start":
        if not os.path.exists(path):
            cmds, saved = ledger.rtk_totals()
            with open(path, "w") as fh:
                json.dump({"cmds": cmds, "saved": saved, "shown": 0}, fh)
        return
    try:
        rec = ledger.build(session)
    except Exception as e:
        note(f"record failed: {e!r}"[:200])
        return
    gain = ledger.gained(rec)
    try:
        with open(path) as fh:
            st = json.load(fh)
    except Exception:
        st = {"shown": 0}
    total = rec["compression"]["saved"] + rec["device"]["saved"]
    n = rec["compression"]["commands"] + rec["device"]["outputs"]
    kept = rec["delegation"]["kept"]
    if (total <= 0 and kept <= 0) or (total, kept) == tuple(st.get("shown") or (0, 0)):
        note(f"silent (gain {gain})")
        return
    st["shown"] = [total, kept]
    with open(path, "w") as fh:
        json.dump(st, fh)

    msg = f"RTK saved ~{fmt(total)} tokens this task ({n} commands compressed)"
    if kept:
        msg += f"; helpers kept ~{fmt(kept)} tokens out of the main context"
    note(f"message (gain {gain})")
    print(json.dumps({"systemMessage": msg + " (/rtk-gain for the details)"}))


if __name__ == "__main__":
    main()

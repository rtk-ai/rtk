import json, os, re, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pipeline as pl

SILENT = ("rtk_task_setup", "rtk_gain", "rtk_laya", "rtk_status")
WITH_RECORD = ("rtk_gain",)


def allowed(tool, args):
    if tool in SILENT:
        return True
    if tool == "rtk_pipelines":
        if (args.get("action") or "list") != "add":
            return True
        return re.fullmatch(r"[\w.-]+", str(args.get("source") or args.get("name") or "")) is not None
    return False


def log(ev, tool, what):
    try:
        os.makedirs(pl.STATE, exist_ok=True)
        with open(os.path.join(pl.STATE, "allow.log"), "a") as fh:
            fh.write(f"{ev.get('session_id', 'x')}\t{time.strftime('%H:%M:%S')}\t{tool}\t{what}\n")
    except OSError:
        pass


def main():
    try:
        ev = json.load(sys.stdin)
    except ValueError:
        return
    name = ev.get("tool_name") or ""
    if "plugin_rtk_rtk-desktop__" not in name:
        return
    tool, args = name.rsplit("__", 1)[-1], ev.get("tool_input") or {}
    if not allowed(tool, args):
        log(ev, tool, "ask")
        return
    out = {"hookEventName": "PreToolUse", "permissionDecision": "allow", "permissionDecisionReason": "rtk plugin tool"}
    what = "allow"
    if tool in WITH_RECORD:
        try:
            import ledger
            rec = ledger.build(ev.get("session_id") or "x")
            rec["via"] = "hook"
            out["updatedInput"] = dict(args, ledger=rec)
            what = "allow+record"
        except Exception as e:
            what = f"allow, record failed: {e!r}"[:120]
    log(ev, tool, what)
    print(json.dumps({"hookSpecificOutput": out}))


if __name__ == "__main__":
    main()

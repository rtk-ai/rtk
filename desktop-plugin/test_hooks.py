import json, os, shutil, subprocess, sys, tempfile, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
RTK_BIN = os.environ.get("RTK_BIN") or os.path.join(HERE, ".rtk-musl", "rtk")
if not os.path.isfile(RTK_BIN):
    sys.exit(f"set RTK_BIN to an rtk binary (no file at {RTK_BIN})")
ROOT = tempfile.mkdtemp(prefix="rtk-hooks-test-")
PLUGIN = os.path.join(ROOT, "plugin")
HOME = os.path.join(ROOT, "home")
shutil.copytree(os.path.join(HERE, "plugin"), PLUGIN, ignore=shutil.ignore_patterns("desktop", "runtime", "__pycache__"))
os.makedirs(os.path.join(PLUGIN, "runtime"))
shutil.copy(RTK_BIN, os.path.join(PLUGIN, "runtime", "rtk"))
os.chmod(os.path.join(PLUGIN, "runtime", "rtk"), 0o755)
for sh in os.listdir(os.path.join(PLUGIN, "hooks")):
    if sh.endswith(".sh"):
        p = os.path.join(PLUGIN, "hooks", sh)
        with open(p) as fh:
            lines = fh.read().split("\n")
        with open(p, "w") as fh:
            fh.write("\n".join(l for l in lines if "uname -s" not in l))
os.makedirs(os.path.join(HOME, ".claude", "projects", "-home-claude"), exist_ok=True)
os.environ.update(HOME=HOME, CLAUDE_PLUGIN_ROOT=PLUGIN, RTK_CONFIG_DIR=os.path.join(HOME, "rtkcfg"))
sys.path.insert(0, os.path.join(PLUGIN, "hooks"))
import pipeline as pl
import ledger
import summary

STATE = os.path.join(ROOT, "state")
pl.STATE, pl.OFF_CFG, pl.LABELS, pl.SETUP = STATE, os.path.join(STATE, "cfg-off"), os.path.join(STATE, "labels.json"), os.path.join(STATE, "setup.json")
pl.DEVICE_LOG = os.path.join(ROOT, "device.log")
ledger.START = os.path.join(ROOT, "start")
os.makedirs(STATE, exist_ok=True)
SID = "11111111-2222-3333-4444-555555555555"
PROJ = os.path.join(HOME, ".claude", "projects", "-home-claude")
SETUP = {"done": True, "active": ["default-bash-compress", "explore-first"], "os": "darwin"}
HOOKS = os.path.join(PLUGIN, "hooks")
PATCH = ("import os, sys; sys.path.insert(0, %r); import pipeline as pl, ledger; pl.STATE, pl.SETUP, pl.LABELS, pl.OFF_CFG, pl.DEVICE_LOG = %r, %r, %r, %r, %r; ledger.START = %r; "
         % (HOOKS, STATE, pl.SETUP, pl.LABELS, pl.OFF_CFG, pl.DEVICE_LOG, ledger.START))


def usage(model, inp=10, w1h=0, w5m=0, rd=0, out=0):
    return {"model": model, "usage": {"input_tokens": inp, "cache_creation": {"ephemeral_1h_input_tokens": w1h, "ephemeral_5m_input_tokens": w5m},
                                      "cache_read_input_tokens": rd, "output_tokens": out}}


def write_transcripts(sub_read_chars=100000):
    main = [
        {"type": "user", "timestamp": "2026-11-02T10:00:00Z", "message": {"role": "user", "content": [{"type": "text", "text": "<system-reminder>x</system-reminder>"}, {"type": "text", "text": "Add a unit test for split_compound_command"}]}},
        {"type": "assistant", "timestamp": "2026-11-02T10:00:05Z", "message": {"id": "m1", **usage("claude-opus-5-5", w1h=20000, rd=900000, out=3000),
                                                                             "content": [{"type": "tool_use", "id": "t1", "name": pl.DEV, "input": {"command": "cd $HOME/mnt/x && git status"}}]}},
        {"type": "user", "timestamp": "2026-11-02T10:00:06Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "On branch main\n" * 50}]}},
        {"type": "assistant", "timestamp": "2026-11-02T10:00:10Z", "message": {"id": "m2", **usage("claude-opus-5-5", w1h=700, rd=920000, out=700),
                                                                             "content": [{"type": "tool_use", "id": "t2", "name": "Agent", "input": {"subagent_type": "Explore", "model": "haiku", "prompt": "find x"}}]}},
        {"type": "assistant", "timestamp": "2026-11-02T10:01:10Z", "message": {"id": "m3", **usage("claude-opus-5-5", out=100), "content": [{"type": "text", "text": "Done."}]}},
        {"type": "assistant", "timestamp": "2026-11-02T10:01:11Z", "message": {"id": "m4", **usage("claude-sonnet-4-6", inp=0, out=0), "content": [{"type": "text", "text": ""}]}},
    ]
    sub = [
        {"type": "user", "isSidechain": True, "timestamp": "2026-11-02T10:00:11Z", "message": {"role": "user", "content": "find x"}},
        {"type": "assistant", "isSidechain": True, "timestamp": "2026-11-02T10:00:12Z", "message": {"id": "s1", **usage("claude-haiku-5-5", w5m=60000, rd=200000, out=2000),
                                                                                                   "content": [{"type": "tool_use", "id": "u1", "name": pl.DEV, "input": {"command": "cd $HOME/mnt/x && grep -rn x src"}}]}},
        {"type": "user", "isSidechain": True, "timestamp": "2026-11-02T10:00:13Z", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "u1", "content": [{"type": "text", "text": "x" * sub_read_chars}]}]}},
        {"type": "assistant", "isSidechain": True, "timestamp": "2026-11-02T10:00:20Z", "message": {"id": "s2", **usage("claude-haiku-5-5", rd=260000, out=3000), "content": [{"type": "text", "text": "report " * 400}]}},
    ]
    os.makedirs(os.path.join(PROJ, SID, "subagents"), exist_ok=True)
    with open(os.path.join(PROJ, SID + ".jsonl"), "w") as fh:
        fh.writelines(json.dumps(e) + "\n" for e in main)
    with open(os.path.join(PROJ, SID, "subagents", "agent-a1.jsonl"), "w") as fh:
        fh.writelines(json.dumps(e) + "\n" for e in sub)


def run_hook(module, event, argv=()):
    code = PATCH + f"import {module}; " + (f"{module}.LOG = {os.path.join(STATE, 'pre.log')!r}; " if module == "pre" else "") + f"{module}.main()"
    r = subprocess.run([sys.executable, "-c", code, *argv], input=json.dumps(event), capture_output=True, text=True, env=dict(os.environ))
    assert r.stderr == "", r.stderr
    return json.loads(r.stdout) if r.stdout.strip() else None


class Ledger(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        write_transcripts()
        with open(pl.SETUP, "w") as fh:
            json.dump(SETUP, fh)
        with open(pl.DEVICE_LOG, "a") as fh:
            fh.write(f"{SID}\t10:00:06\tok\tgit-status\t4000\t1000\tgit status\n")
            fh.write(f"other\t10:00:06\tok\tgrep\t9000\t1000\tgrep\n")

    def test_record(self):
        rec = ledger.build(SID)
        self.assertEqual(rec["sid"], SID)
        self.assertTrue(rec["folder"])
        self.assertEqual(rec["model"], "opus-5.5")
        self.assertEqual(rec["device"], {"outputs": 1, "saved": 750})
        h = rec["delegation"]["helpers"]
        self.assertEqual(len(h), 1)
        self.assertEqual((h[0]["type"], h[0]["model"]), ("Explore", "haiku-5.5"))
        self.assertEqual((h[0]["read"], h[0]["wrote"], h[0]["report"], h[0]["kept"]), (25000, 5000, 700, 29300))
        self.assertAlmostEqual(h[0]["usd"], (60000 * 0.125 + 460000 * 0.01 + 5000 * 0.5 + 20 * 0.1) / 1e6, places=4)
        self.assertAlmostEqual(rec["cost"]["main_usd"], (20700 * 8 + 1820000 * 0.2 + 3800 * 20 + 30 * 4) / 1e6, places=3)
        self.assertEqual(set(rec["cost"]["by_model"]), {"opus-5.5", "haiku-5.5", "sonnet-4.6"})
        self.assertEqual(ledger.gained(rec), 750 + 29300)
        self.assertEqual(rec["compression"]["commands"], 0)

    def test_stop_hook_messages_once_and_never_calls_a_tool(self):
        p = os.path.join(ledger.START, SID + ".json")
        if os.path.exists(p):
            os.remove(p)
        ev = {"session_id": SID, "stop_hook_active": False}
        o = run_hook("summary", ev, ["stop"])
        self.assertNotIn("decision", o)
        self.assertIn("RTK saved ~750 tokens this task (1 commands compressed); helpers kept ~29.3K tokens", o["systemMessage"])
        self.assertIsNone(run_hook("summary", ev, ["stop"]))
        with open(os.path.join(STATE, "stop.log")) as fh:
            log = fh.read()
        self.assertIn("message (gain 30050)", log)
        self.assertIn("silent (gain 30050)", log)


class PreHook(unittest.TestCase):
    def run_pre(self, command):
        ev = {"session_id": SID, "transcript_path": os.path.join(PROJ, SID + ".jsonl"), "cwd": "/home/claude", "hook_event_name": "PreToolUse",
              "tool_name": pl.DEV, "tool_input": {"command": command, "description": "x"}, "tool_use_id": "t9"}
        return run_hook("pre", ev)

    def test_folder_commands_are_never_rewritten(self):
        write_transcripts()
        with open(pl.SETUP, "w") as fh:
            json.dump(SETUP, fh)
        self.assertIsNone(self.run_pre("cd $HOME/mnt/x\ngit status"))
        self.assertIsNone(self.run_pre("python3 scripts/x.py"))
        with open(os.path.join(STATE, "pre.log")) as fh:
            log = fh.read()
        self.assertIn("rewrite-dropped", log)
        self.assertIn("\tpass\t", log)


class Allow(unittest.TestCase):

    def run_allow(self, tool, args, full=False):
        ev = {"session_id": SID, "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": args}
        out = run_hook("allow", ev)
        out = out["hookSpecificOutput"] if out else None
        return out if full else (out["permissionDecision"] if out else None)

    def test_decisions(self):
        cowork = "mcp__remote-devices__plugin_rtk_rtk-desktop__"
        self.assertEqual(self.run_allow(cowork + "rtk_task_setup", {"state": "x"}), "allow")
        self.assertIsNone(self.run_allow(cowork + "rtk_task_done", {"ledger": {}}))
        self.assertEqual(self.run_allow(cowork + "rtk_gain", {}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_status", {}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_laya", {"action": "off"}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_pipelines", {"action": "list"}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_pipelines", {"action": "search", "query": "laya"}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_pipelines", {"action": "reset"}), "allow")
        self.assertEqual(self.run_allow(cowork + "rtk_pipelines", {"action": "add", "source": "laya-tiered"}), "allow")
        self.assertIsNone(self.run_allow(cowork + "rtk_pipelines", {"action": "add", "source": "github:someone/repo"}))
        self.assertIsNone(self.run_allow(cowork + "rtk_pipelines", {"action": "add", "source": "https://x.example/p.toml"}))
        self.assertIsNone(self.run_allow(cowork + "rtk_pipelines", {"action": "add", "source": "/tmp/p.toml"}))
        self.assertIsNone(self.run_allow("mcp__other__rtk_gain", {}))

    def test_record_attached_to_gain(self):
        write_transcripts()
        with open(pl.SETUP, "w") as fh:
            json.dump(SETUP, fh)
        cowork = "mcp__remote-devices__plugin_rtk_rtk-desktop__"
        for args in ({}, {"ledger": {"v": 1, "sid": "stale"}}):
            out = self.run_allow(cowork + "rtk_gain", args, full=True)
            self.assertEqual(out["permissionDecision"], "allow")
            rec = out["updatedInput"]["ledger"]
            self.assertEqual((rec["sid"], rec["via"], rec["v"]), (SID, "hook", 1))
            self.assertEqual(rec["delegation"]["kept"], 29300)
        self.assertNotIn("updatedInput", self.run_allow(cowork + "rtk_status", {}, full=True))
        with open(os.path.join(STATE, "allow.log")) as fh:
            self.assertIn("rtk_gain\tallow+record", fh.read())

    def test_report_is_markdown(self):
        r = subprocess.run([sys.executable, "-c", PATCH + "pl.report()"], capture_output=True, text=True, env=dict(os.environ))
        self.assertEqual(r.stderr, "")
        self.assertRegex(r.stdout, r"^## RTK report\n- pipelines: .*rtk \d.* · wall-clock 1m11s\n\n\| Model \| Input \|")
        self.assertIn("| claude-opus-5-5 | 30 | 0 | 20.7K | 1.82M | 3.8K | 0.606 |", r.stdout)
        self.assertIn("| **total** | | | | | | **0.6", r.stdout)
        self.assertIn("- sub-agents: Explore on haiku ×1", r.stdout)
        self.assertRegex(r.stdout, r"\n- plugin tools: .*rtk_gain allow\+record \d")
        self.assertNotIn("sonnet-4-6", r.stdout)


class Rtk(unittest.TestCase):

    def test_rtk_commands(self):
        rtk = os.path.join(PLUGIN, "runtime", "rtk")
        self.assertRegex(subprocess.run([rtk, "--version"], capture_output=True, text=True).stdout, r"rtk \d+\.\d+\.\d+")
        gain = json.loads(subprocess.run([rtk, "gain", "--format", "json"], capture_output=True, text=True).stdout)
        self.assertIn("total_saved", gain["summary"])
        ev = {"session_id": "s", "tool_name": "Bash", "tool_input": {"command": "cd /tmp && git status"}, "cwd": "/tmp", "hook_event_name": "PreToolUse"}
        out = subprocess.run([rtk, "hook", "claude"], input=json.dumps(ev), capture_output=True, text=True).stdout
        self.assertEqual(json.loads(out)["hookSpecificOutput"]["updatedInput"]["command"], "cd /tmp && rtk git status")
        piped = subprocess.run([rtk, "pipe", "-f", "grep"], input="a.rs:1:x\n" * 300, capture_output=True, text=True)
        self.assertEqual(piped.returncode, 0)


if __name__ == "__main__":
    print("scratch:", ROOT, "rtk:", RTK_BIN)
    unittest.main(verbosity=1)

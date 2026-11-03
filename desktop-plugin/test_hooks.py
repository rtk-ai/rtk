import atexit, json, os, shutil, subprocess, sys, tempfile, threading, unittest

HERE = os.path.dirname(os.path.abspath(__file__))
RTK_BIN = os.environ.get("RTK_BIN") or os.path.join(HERE, ".rtk-musl", "rtk")
if not os.path.isfile(RTK_BIN):
    sys.exit(f"set RTK_BIN to an rtk binary (no file at {RTK_BIN})")
ROOT = tempfile.mkdtemp(prefix="rtk-hooks-test-")
if not os.environ.get("KEEP"):
    atexit.register(shutil.rmtree, ROOT, True)
PLUGIN = os.path.join(ROOT, "plugin")
HOME = os.path.join(ROOT, "home")
FIXTURE = os.path.join(HERE, "desktop-src", "test", "fixtures", "laya-tiered.toml")
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
STATE = os.path.join(ROOT, "state")
os.environ.update(HOME=HOME, CLAUDE_PLUGIN_ROOT=PLUGIN, RTK_CONFIG_DIR=os.path.join(HOME, "rtkcfg"), RTK_COWORK_STATE=STATE)
sys.path.insert(0, os.path.join(PLUGIN, "hooks"))
import pipeline as pl
import ledger
import summary

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
    r = subprocess.run([sys.executable, "-c", code, *argv], input=json.dumps(event) if not isinstance(event, str) else event, capture_output=True, text=True, env=dict(os.environ))
    assert r.stderr == "", r.stderr
    return json.loads(r.stdout) if r.stdout.strip() else None


DSID = "dddddddd-2222-3333-4444-555555555555"


def device_event(command, text):
    return {"session_id": DSID, "hook_event_name": "PostToolUse", "tool_name": pl.DEV, "tool_input": {"command": command},
            "tool_response": {"content": [{"type": "text", "text": text}]}, "transcript_path": os.path.join(PROJ, SID + ".jsonl")}


def device_log():
    with open(pl.DEVICE_LOG) as fh:
        return fh.read()


def start_file():
    cmds, saved = ledger.rtk_totals()
    os.makedirs(ledger.START, exist_ok=True)
    with open(os.path.join(ledger.START, SID + ".json"), "w") as fh:
        json.dump({"cmds": cmds, "saved": saved, "shown": 0}, fh)


class Ledger(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        write_transcripts()
        start_file()
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
        start_file()
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


class DevicePost(unittest.TestCase):
    def run_device(self, command, text):
        before = len(device_log().splitlines()) if os.path.exists(pl.DEVICE_LOG) else 0
        out = run_hook("device_post", device_event(command, text))
        status = [l.split("\t")[2:4] for l in device_log().splitlines()[before:]]
        return (out["hookSpecificOutput"]["updatedMCPToolOutput"][0]["text"] if out else None), status

    def test_pytest_failure_is_compressed_with_a_tag(self):
        raw = "=" * 29 + " test session starts " + "=" * 30 + "\n" + "".join(f"tests/test_m{i}.py ..F.{' ' * 50}[ 50%]\n" for i in range(30))
        raw += "=" * 35 + " FAILURES " + "=" * 35 + "\n___ test_x ___\n    assert 1 == 2\nE   assert 1 == 2\ntests/test_m1.py:12: AssertionError\n" + "=" * 25 + " 1 failed, 119 passed in 2.31s " + "=" * 24 + "\n"
        text, status = self.run_device("cd $HOME/mnt/x && pytest -q", raw)
        self.assertRegex(text, r"^\[rtk: \d+ KB -> \d+ B \(-\d+%\)\]\n")
        self.assertIn("1 failed", text)
        self.assertIn("test_x", text)
        self.assertEqual(status, [["ok", "pytest"]])

    def test_listings_are_never_summarised_away(self):
        pytest_list = "".join(f"tests/test_module_{i % 9}.py::test_case_{i}\n" for i in range(120))
        go_list = "".join(f"TestHandler{i}\n" for i in range(120)) + "ok      example.com/pkg 0.004s\n"
        tsc_list = "".join(f"/home/claude/project/src/component_{i}.tsx\n" for i in range(60))
        cargo_list = "".join(f"module::tests::case_{i}: test\n" for i in range(60)) + "\n60 tests, 0 benchmarks\n"
        for command, raw, keep in (("pytest --collect-only -q", pytest_list, "test_case_7"), ("go test -list . ./...", go_list, "TestHandler7"),
                                   ("npx tsc --listFiles --noEmit", tsc_list, "component_7"), ("cargo test -- --list", cargo_list, "case_7"),
                                   ("pytest tests/ -k handler", pytest_list, "test_case_7")):
            text, status = self.run_device(command, raw)
            self.assertTrue(text is None or keep in text, (command, text))
            self.assertNotIn("ok", [s[0] for s in status], (command, status))
            self.assertTrue(all(s[0].startswith("skip-") for s in status), (command, status))
        self.assertEqual(status[0], ["skip-marker", "pytest"])

    def test_a_countless_one_line_summary_never_replaces_a_long_output(self):
        raw = "".join(f"=== RUN   TestCase{i}\n--- PASS: TestCase{i} (0.00s)\n" for i in range(40)) + "PASS\nok  \tgithub.com/x/pkg\t0.512s\n"
        text, status = self.run_device("go test -v ./pkg", raw)
        self.assertIn(status[0][0], ("ok", "skip-floor", "skip-ratio"), status)
        if status[0][0] == "ok":
            self.assertRegex(text.split("\n", 1)[1], r"\d")
        else:
            self.assertTrue(text is None or "TestCase7" in text)

    def test_malformed_events_never_crash_a_hook(self):
        events = ["[]", '"x"', "7", {"tool_input": 5}, {"tool_input": [1], "tool_name": "Bash", "session_id": 7}, {"tool_input": {"command": 5}, "tool_name": pl.DEV, "tool_response": {"content": [{"type": "text", "text": "x" * 2000}]}},
                  {"tool_name": "mcp__remote-devices__plugin_rtk_rtk-desktop__rtk_pipelines", "tool_input": "add"}, {"tool_response": [{"text": 5}, 3], "tool_name": "x"}]
        for module in ("device_post", "pre", "allow", "summary", "setup_post"):
            for ev in events:
                out = run_hook(module, ev, ["stop"] if module == "summary" else ())
                self.assertTrue(out is None or isinstance(out, dict), (module, ev, out))

    def test_small_piped_and_unknown_outputs_pass(self):

        text, status = self.run_device("pytest -q", "1 passed\n")
        self.assertIsNone(text)
        self.assertEqual(status, [["skip-small", "pytest"]])
        for command in ("pytest -q | tail -5", "python3 scripts/x.py"):
            text, status = self.run_device(command, "x\n" * 600)
            self.assertIn([s[0] for s in status], (["skip-nofilter"], ["ok-pipeline"]), (command, status))
            self.assertTrue(text is None or "x600" in text, (command, text))

        self.assertIsNone(run_hook("device_post", "null"))
        self.assertIsNone(run_hook("device_post", "not json"))

    def test_saved_output_outside_tool_results_is_left_alone(self):
        ev = device_event("grep -rn x src", "Output has been saved to /etc/passwd.txt")
        ev["tool_response"] = "Output has been saved to /etc/passwd.txt"
        self.assertIsNone(run_hook("device_post", ev))
        self.assertIn("skip-saved", device_log())


class SetupPost(unittest.TestCase):
    def setUp(self):
        for f in os.listdir(pl.pipelines_dir()) if os.path.isdir(pl.pipelines_dir()) else []:
            os.remove(os.path.join(pl.pipelines_dir(), f))
        for name in ("installed", "labels.json"):
            if os.path.exists(os.path.join(STATE, name)):
                os.remove(os.path.join(STATE, name))

    def run_setup(self, data):
        ev = {"session_id": SID, "hook_event_name": "PostToolUse", "tool_name": "mcp__remote-devices__plugin_rtk_rtk-desktop__rtk_task_setup",
              "tool_input": {"state": "x"}, "tool_response": {"content": [{"type": "text", "text": json.dumps(data)}]}}
        out = run_hook("setup_post", ev)
        return out["hookSpecificOutput"]["updatedMCPToolOutput"][0]["text"] if out else None

    def test_pipelines_dir_follows_rtk_config_dir(self):
        self.assertEqual(pl.pipelines_dir(), os.path.join(HOME, "rtkcfg", "pipelines"))

    def test_setup_installs_the_active_pipelines_only(self):
        with open(FIXTURE) as fh:
            toml = fh.read()
        msg = self.run_setup({"active": ["default-bash-compress", "laya-tiered"], "pipelines": {"laya-tiered": toml}, "os": "darwin", "notice": "Tell the user once: hi."})
        self.assertEqual(msg, "Tell the user once: hi. Carry on with the task; do not mention this step otherwise.")
        self.assertEqual(sorted(os.listdir(pl.pipelines_dir())), ["default-bash-compress.toml", "laya-tiered.toml"])
        setup = pl.read_json(pl.SETUP, {})
        self.assertEqual((setup["done"], setup["active"], setup["failed"], setup["os"]), (True, ["default-bash-compress", "laya-tiered"], [], "darwin"))
        self.assertFalse(os.path.exists(pl.LABELS))

    def test_setup_reports_a_pipeline_it_could_not_install(self):
        msg = self.run_setup({"active": ["default-bash-compress", "explore-first", "broken"], "pipelines": {"broken": "not a pipeline"}})
        self.assertIn("could not be installed: broken", msg)
        self.assertEqual(sorted(os.listdir(pl.pipelines_dir())), ["default-bash-compress.toml", "explore-first.toml"])

    def test_setup_survives_a_malformed_response(self):
        self.assertIsNone(self.run_setup(["not", "an", "object"]))
        self.assertIsNone(self.run_setup("plain text"))
        msg = self.run_setup({"active": "explore-first", "pipelines": "oops"})
        self.assertEqual(msg, "RTK keeps its default pipelines for this task. Carry on with the task; do not mention this step otherwise.")
        with open(os.path.join(STATE, "setup.log")) as fh:
            self.assertIn("apply_setup failed", fh.read())

    def test_labels_start_the_bridge_once(self):
        calls = []
        os.makedirs(STATE, exist_ok=True)
        with open(os.path.join(STATE, "installed"), "w") as fh:
            fh.write("x\n")
        real = pl.subprocess.Popen
        pl.subprocess.Popen = lambda *a, **k: calls.append(a[0])
        try:

            pl.apply_setup({"active": ["default-bash-compress"], "labels": {"kind": {"choice": "write", "confidence": 0.9}}})
        finally:
            pl.subprocess.Popen = real
        self.assertEqual(len(calls), 1)
        self.assertTrue(calls[0][1].endswith("laya_bridge.py"))
        self.assertEqual(pl.read_json(pl.LABELS, {}), {"kind": {"choice": "write", "confidence": 0.9}})


class Transcript(unittest.TestCase):
    def test_parallel_hooks_share_a_transcript_without_racing(self):
        write_transcripts()
        src = os.path.join(PROJ, SID + ".jsonl")
        errors = []

        def one():
            try:
                for _ in range(20):
                    path, asked, goal, calls = pl.present(src, SID, None)
                    assert os.path.exists(path) and goal.startswith("Add a unit test") and calls
            except Exception as e:
                errors.append(repr(e))
        threads = [threading.Thread(target=one) for _ in range(6)]
        [t.start() for t in threads]
        [t.join() for t in threads]
        self.assertEqual(errors, [])
        self.assertEqual([f for f in os.listdir(STATE) if f.endswith(".tmp")], [])

    def test_odd_session_ids_stay_inside_the_state_dir(self):
        write_transcripts()
        path = pl.present(os.path.join(PROJ, SID + ".jsonl"), "../../etc/passwd", None)[0]
        self.assertEqual(os.path.dirname(path), STATE)
        self.assertNotIn("..", os.path.basename(path))

    @unittest.skipIf(pl.cowork(), "inside a Cowork workspace")
    def test_commands_stay_quiet_outside_cowork(self):
        for arg in ("report", "off"):
            r = subprocess.run([sys.executable, os.path.join(HOOKS, "pipeline.py"), arg], capture_output=True, text=True, env=dict(os.environ))
            self.assertEqual((r.returncode, r.stdout, r.stderr), (0, "This RTK command works in Claude Desktop tasks only.\n", ""))
        self.assertFalse(os.path.exists(os.path.join(STATE, "pipeline")))


class RunSh(unittest.TestCase):
    def test_off_mode_points_rtk_at_an_empty_config(self):
        os.makedirs(STATE, exist_ok=True)
        with open(os.path.join(STATE, "pipeline"), "w") as fh:
            fh.write("off\n")
        try:
            ev = {"session_id": SID, "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {"command": "ls"}, "tool_response": {"stdout": "a\nb\n", "stderr": ""}, "cwd": "/tmp"}
            env = {k: v for k, v in os.environ.items() if k != "RTK_CONFIG_DIR"}
            r = subprocess.run(["sh", os.path.join(HOOKS, "run.sh")], input=json.dumps(ev), capture_output=True, text=True, env=env)
            self.assertEqual(r.stderr, "")
            self.assertTrue(os.path.exists(os.path.join(STATE, "cfg-off", "config.toml")))
            if r.stdout.strip():
                json.loads(r.stdout)
        finally:
            os.remove(os.path.join(STATE, "pipeline"))


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

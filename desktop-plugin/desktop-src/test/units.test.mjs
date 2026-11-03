import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { dataDir, Store } from "../server/lib/store.mjs";
import { candidates, findInstalled } from "../server/lib/rtklocal.mjs";
import { parseToml, pipelineName, layaQuestions, row, usesLaya, handsOff } from "../server/lib/pipelines.mjs";
import { recordTask, allTasks, gainText, taskText, validRecord, cell } from "../server/lib/ledger.mjs";
import { allowedUrl, get, pipelineUrl } from "../server/lib/github.mjs";
import { npmCommand } from "../server/lib/laya.mjs";

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), "rtk-desktop-test-"));
const HERE = path.dirname(fileURLToPath(import.meta.url));

test("data dir per OS", () => {
  assert.equal(dataDir("darwin", {}, "/Users/u"), "/Users/u/Library/Application Support/rtk-desktop");
  assert.equal(dataDir("linux", {}, "/home/u"), "/home/u/.local/share/rtk-desktop");
  assert.equal(dataDir("linux", { XDG_DATA_HOME: "/x" }, "/home/u"), "/x/rtk-desktop");
  assert.equal(dataDir("win32", { APPDATA: "C:\\Users\\u\\AppData\\Roaming" }, "C:\\Users\\u"), path.join("C:\\Users\\u\\AppData\\Roaming", "rtk-desktop"));
  assert.equal(dataDir("win32", {}, "C:\\Users\\u"), path.join("C:\\Users\\u", "AppData", "Roaming", "rtk-desktop"));
});

test("installed rtk candidates follow the OS, and the first one answering --version wins", () => {
  const win = candidates("win32", { PATH: "C:\\a;C:\\b", LOCALAPPDATA: "C:\\L" }, "C:\\Users\\u");
  assert.ok(win.every((p) => p.endsWith("rtk.exe")));
  assert.ok(win.includes(path.join("C:\\a", "rtk.exe")) && win.includes(path.join("C:\\L", "Programs", "rtk", "rtk.exe")));
  const mac = candidates("darwin", { PATH: "/usr/bin:/opt/x" }, "/Users/u");
  assert.ok(mac.includes("/opt/x/rtk") && mac.includes("/Users/u/.local/bin/rtk") && mac.includes("/opt/homebrew/bin/rtk"));
  assert.equal(new Set(mac).size, mac.length);
  const d = tmp();
  fs.mkdirSync(path.join(d, "bin"));
  fs.writeFileSync(path.join(d, "bin", "rtk"), "#!/bin/sh\necho 'rtk 1.2.3'\n");
  fs.chmodSync(path.join(d, "bin", "rtk"), 0o755);
  assert.deepEqual(findInstalled({ platform: "darwin", env: { PATH: path.join(d, "bin") }, home: d, login: false }), { path: path.join(d, "bin", "rtk"), version: "1.2.3" });
  assert.equal(findInstalled({ platform: "darwin", env: { PATH: "/nonexistent" }, home: d, login: false }), null);
});

test("pipeline sources", () => {
  assert.equal(pipelineUrl("github:o/r"), "https://raw.githubusercontent.com/o/r/HEAD/pipeline.toml");
  assert.equal(pipelineUrl("https://github.com/o/r@abc123"), "https://raw.githubusercontent.com/o/r/abc123/pipeline.toml");
  assert.equal(pipelineUrl("github:o/r@release/v1.2"), "https://raw.githubusercontent.com/o/r/release/v1.2/pipeline.toml");
  assert.equal(pipelineUrl("laya-tiered"), null);
  assert.equal(pipelineUrl("github:o/r@../../evil/repo/main"), null);
  assert.equal(pipelineUrl("github:o/r@./main"), null);
  assert.equal(pipelineUrl("github:../.."), null);
  assert.ok(allowedUrl(new URL("https://raw.githubusercontent.com/o/r/HEAD/pipeline.toml")));
  assert.ok(allowedUrl(new URL("http://127.0.0.1:8080/x")));
  assert.ok(!allowedUrl(new URL("http://example.com/x")));
  assert.ok(!allowedUrl(new URL("ftp://example.com/x")));
});

test("downloads stay on https, stop at the size cap and give up on redirect loops", async (t) => {
  const srv = http.createServer((req, res) => {
    res.on("error", () => {});
    if (req.url === "/small") return res.end("ok " + req.headers["user-agent"]);
    if (req.url === "/hop") { res.writeHead(302, { location: "/small" }); return res.end(); }
    if (req.url === "/down") { res.writeHead(302, { location: "http://example.com/x" }); return res.end(); }
    if (req.url === "/loop") { res.writeHead(302, { location: "/loop" }); return res.end(); }
    if (req.url === "/big") {
      res.writeHead(200, { "content-type": "text/plain" });
      let n = 0;
      const pump = () => { while (n < 64 && res.write("x".repeat(65536))) n++; if (n < 64) res.once("drain", pump); else res.end(); };
      return pump();
    }
    res.writeHead(404);
    res.end();
  });
  await new Promise((r) => srv.listen(0, "127.0.0.1", r));
  t.after(() => { srv.closeAllConnections(); srv.close(); });
  const base = `http://127.0.0.1:${srv.address().port}`;
  assert.equal(await get(base + "/small"), "ok rtk-desktop");
  assert.equal(await get(base + "/hop"), "ok rtk-desktop");
  await assert.rejects(get(base + "/big", { max: 100000 }), /too large \(over 100000 bytes\)/);
  await assert.rejects(get(base + "/down"), /refused http:\/\/example\.com/);
  await assert.rejects(get(base + "/loop"), /too many redirects/);
  await assert.rejects(get("http://example.com/x"), /refused http:\/\/example\.com/);
  await assert.rejects(get(base + "/missing"), /HTTP 404/);
});

test("npm command line per OS", () => {
  const win = npmCommand("win32", "C:\\Program Files\\nodejs", ["install", "--no-audit", "@receptron/laya"], { ComSpec: "C:\\Windows\\System32\\cmd.exe" });
  assert.equal(win.file, "C:\\Windows\\System32\\cmd.exe");
  assert.deepEqual(win.args, ["/d", "/s", "/c", '""C:\\Program Files\\nodejs\\npm.cmd" "install" "--no-audit" "@receptron/laya""']);
  assert.equal(win.options.windowsVerbatimArguments, true);
  assert.deepEqual(npmCommand("darwin", "/opt/homebrew/bin", ["init", "-y"]), { file: "/opt/homebrew/bin/npm", args: ["init", "-y"], options: {} });
});

test("toml reader handles comments, multi-line strings, quoted names and arrays", () => {
  const toml = [
    "[pipeline] # the header",
    'name = "demo" # trailing',
    "tags = [\"a, b\", \"c#d\", 'e']   # commas and hashes inside strings",
    "nested = [[1, 2], [3]]",
    "n = 1_000",
    "neg = -2.5",
    '[classifier."kind"]',
    'question = "choice"',
    'text = """',
    'line one "quoted"',
    "line two \\",
    "   joined",
    '"""',
    "lit = '''",
    "raw \\n kept",
    "'''",
    '# provider = "laya"',
    "[upstream.x]",
    'provider = "laya"',
    "[[hook.before_tool.shell]]",
    'action = "deny"',
    "",
  ].join("\n");
  const sec = parseToml(toml);
  assert.equal(sec.pipeline.name, "demo");
  assert.deepEqual(sec.pipeline.tags, ["a, b", "c#d", "e"]);
  assert.deepEqual(sec.pipeline.nested, [[1, 2], [3]]);
  assert.equal(sec.pipeline.n, 1000);
  assert.equal(sec.pipeline.neg, -2.5);
  assert.equal(sec["classifier.kind"].question, "choice");
  assert.equal(sec["classifier.kind"].text, 'line one "quoted"\nline two joined\n');
  assert.equal(sec["classifier.kind"].lit, "raw \\n kept\n");
  assert.ok(usesLaya(toml) && handsOff(toml));
  assert.ok(!usesLaya('# provider = "laya"\n[pipeline]\nname = "x"\n'));
  assert.ok(!handsOff('[pipeline]\nname = "x"\n# action = "deny"\n'));
  assert.equal(pipelineName('# [pipeline]\n# name = "wrong"\n[pipeline]\nname = "right"\n'), "right");
});

test("toml reader and Laya questions on a copy of the published laya-tiered", () => {
  const sec = parseToml('[a]\nx = "s\\"q"\ny = 1.5\nz = ["p", "q"]\nw = true\n[b.c]\n"k" = \'lit\'\n[[t]]\nn = 1\n');
  assert.deepEqual(sec, { a: { x: 's"q', y: 1.5, z: ["p", "q"], w: true }, "b.c": { k: "lit" } });
  const toml = fs.readFileSync(path.join(HERE, "fixtures", "laya-tiered.toml"), "utf8");
  assert.equal(pipelineName(toml), "laya-tiered");
  assert.ok(usesLaya(toml) && handsOff(toml));
  const q = layaQuestions(toml);
  assert.deepEqual(Object.keys(q), ["kind"]);
  assert.equal(q.kind.type, "choice");
  assert.ok(q.kind.criteria.write && q.kind.instructions.length > 20);
  const bundled = fs.readFileSync(path.join(HERE, "..", "..", "plugin", "pipeline", "explore-first.toml"), "utf8");
  assert.equal(pipelineName(bundled), "explore-first");
  assert.ok(!usesLaya(bundled) && handsOff(bundled));
});

test("browse row", () => {
  const e = { name: "laya-tiered", owner: "rtk-ai", tier: "text", rtk: "v1.0.1", suite: "SkillsBench", model: "claude-opus-5-5", baseline: "stock Claude Code", trials: 44, cost: { baseline: 4.312, pipeline: 3.44 }, quality: { metric: "reward", baseline: 0.66, pipeline: 0.64 } };
  assert.equal(row(e, ["laya-tiered"]), "| **laya-tiered** ✅ in use | 🟢 -20% vs stock Claude Code | reward 0.66 → 0.64 | SkillsBench, claude-opus-5-5, 44 trials/arm | text | rtk-ai | `/rtk-pipeline-add laya-tiered` |");
  assert.match(row({ name: "explore-first", owner: "x" }), /^\| \*\*explore-first\*\* \(bundled default\) \| no benchmark published \|  \|  \| \? \| x \|/);
});

test("task records and the gain text", () => {
  const rec = { v: 1, sid: "abc-1", at: "2026-11-02", pipelines: ["default-bash-compress", "laya-tiered"], laya: { kind: "write" }, compression: { commands: 10, saved: 48200 }, device: { outputs: 2, saved: 800 },
    delegation: { kept: 61000, helpers: [{ type: "Explore", model: "haiku-5.5", read: 10000, wrote: 0, report: 700, kept: 9300, usd: 0.02 }, { type: "general-purpose", model: "sonnet-5.5", read: 3400, wrote: 30300, report: 800, kept: 32900, usd: 0.85 }] },
    cost: { main_usd: 0.54, helpers_usd: 0.74, by_model: { "opus-5.5": 0.54, "sonnet-5.5": 0.85, "haiku-5.5": 0.02, "sonnet-4.6": 0 } }, rules: { "delegate-investigation": 1 } };
  const task = taskText(rec);
  assert.equal(task.split("\n")[0], "| | This task · pipelines: default-bash-compress, laya-tiered · Laya: kind=write |");
  assert.match(task, /\n\| 🗜️ Compression \| 12 commands, \*\*~49\.0K tokens saved\*\* \(10 in the workspace, 2 from the connected folder\) \|\n/);
  assert.match(task, /\n\| 🤝 Delegation \| Explore on haiku-5\.5 read ~10\.0K, reported ~700: \*\*~9\.3K tokens kept out of the main context\*\*, helper cost \$0\.02 \|\n\|  \| general-purpose on sonnet-5\.5 read ~3\.4K, produced ~30\.3K, reported ~800: \*\*~32\.9K tokens kept/);
  assert.match(task, /\n\| 💵 Cost \| sonnet-5\.5 \$0\.85, opus-5\.5 \$0\.54, haiku-5\.5 \$0\.02 \(API list prices, estimate\) \|\n\| 📏 Rules \| delegate-investigation ×1 \|$/);
  assert.ok(!task.includes("sonnet-4.6"));
  const store = new Store(tmp());
  assert.ok(recordTask(store, { v: 1, sid: "old", at: "2026-11-01", compression: { commands: 3, saved: 900 }, cost: { main_usd: 0.2 } }));
  assert.ok(recordTask(store, { ...rec, delegation: { kept: 0, helpers: [] } }));
  assert.ok(recordTask(store, rec));
  assert.ok(!recordTask(store, { sid: "x" }));
  assert.ok(!recordTask(store, { v: 1, sid: "bad sid" }));
  const tasks = allTasks(store);
  assert.equal(tasks.length, 2);
  assert.equal(tasks[0].sid, "old");
  assert.ok(tasks[1].helpers === 2 && tasks[1].saved === 49000 && tasks[1].kept === 61000);
  const txt = gainText(tasks, { saved: 4300000, commands: 1204 }, { path: "/x/rtk", version: "1.1.0" }, rec);
  assert.match(txt, /^## RTK gain\n\n\| \| This task/);
  assert.match(txt, /\n\| \| On this computer \|\n\|---\|---\|\n\| Claude Code \| \*\*~4\.3M tokens saved\*\* over 1,204 commands \(rtk gain\) \|\n/);
  assert.match(txt, /\n\| Desktop tasks \| 2 since 2026-11-01: \*\*~49\.9K tokens compressed\*\* over 15 commands \|\n/);
  assert.match(txt, /\n\| Delegation \| 1 tasks, 2 helpers: \*\*~61\.0K tokens kept out of the main model's context\*\*; helpers \$0\.74 against \$0\.74 for the main model/);
  assert.match(txt, /\n\| Rules fired \| 1 \|\n\n_A task is recorded when `\/rtk-gain` runs in it/);
  assert.match(gainText([], null, null, null), /\n⚠️ The record of this task did not arrive\.\n\n\| \| On this computer \|\n\|---\|---\|\n\| Claude Code \| rtk is not installed here, so no history \|\n\| Desktop tasks \| none recorded yet \|\n/);
  assert.match(gainText([], null, { path: "/x/rtk", version: "0.9.0" }, null), /rtk 0\.9\.0 at \/x\/rtk did not answer/);
});

test("records are bounded and table cells are flattened", () => {
  assert.ok(validRecord({ v: 1, sid: "a", delegation: { helpers: [] } }));
  assert.ok(!validRecord({ v: 1, sid: "a", delegation: { helpers: Array.from({ length: 51 }, () => ({})) } }));
  assert.ok(!validRecord({ v: 1, sid: "a", delegation: { helpers: "many" } }));
  assert.ok(!validRecord({ v: 1, sid: "a", pad: "x".repeat(70000) }));
  assert.ok(!validRecord([1]));
  assert.equal(cell("a|b\nc"), "a b c");
  const text = taskText({ v: 1, sid: "a", pipelines: ["p|q"], laya: { "k|": "v\n" }, delegation: { helpers: [{ type: "Explore|x", model: "haiku\n5.5", read: 1, kept: 1, usd: "nope" }] }, rules: { "r|s": 2 } });
  assert.ok(!/\|q|Explore\|x|haiku\n|k\||r\|s/.test(text), text);
  assert.match(text, /pipelines: p q · Laya: k=v \|/);

  assert.match(text, /Explore x on haiku 5\.5 read ~1, reported ~0/);
  assert.match(text, /helper cost \$0\.00 \|\n\| 📏 Rules \| r s ×2 \|$/);
});

test("store writes are atomic and corrupt files fall back to the default", () => {
  const store = new Store(tmp());
  store.writeJson("a.json", { x: 1 });
  assert.deepEqual(store.readJson("a.json", null), { x: 1 });
  assert.deepEqual(fs.readdirSync(store.dir).filter((f) => f.endsWith(".tmp")), []);
  fs.writeFileSync(store.path("b.json"), "{not json");
  assert.deepEqual(store.readJson("b.json", [1]), [1]);
  assert.match(store.readText("server.log"), /b\.json is not valid JSON/);
  assert.equal(store.readJson("missing.json", "d"), "d");
});


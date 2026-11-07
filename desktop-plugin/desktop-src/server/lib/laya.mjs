import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";

export const IDLE = 1800;
export const PORT = 41733;

export function npmCommand(platform, bin, args, env = process.env) {
  if (platform !== "win32") return { file: path.posix.join(bin, "npm"), args, options: {} };
  const line = [path.win32.join(bin, "npm.cmd"), ...args].map((a) => `"${a}"`).join(" ");

  return { file: env.ComSpec || "cmd.exe", args: ["/d", "/s", "/c", `"${line}"`], options: { windowsVerbatimArguments: true } };
}

export class Laya {
  constructor(store, { platform = process.platform, env = process.env, home = os.homedir(), src = null } = {}) {
    this.store = store;
    this.platform = platform;
    this.env = env;
    this.home = home;
    this.src = src;
    this.port = Number(env.RTK_DESKTOP_LAYA_PORT || PORT);
    this.url = `http://127.0.0.1:${this.port}`;
    this.dir = store.path("laya");
    this.logPath = store.path("laya.log");
    this.phase = "stopped";
    this.error = "";
    this.used = 0;
    this.proc = null;
    this.fix = {
      node: "to use their Laya-based RTK pipeline, install Node.js 20 or newer (nodejs.org) and restart Claude Desktop; RTK then sets up its Laya classifier by itself (one-time download of about 1.7 GB)",
      setup: "RTK is setting up its Laya classifier on this computer for the first time (about 1.7 GB to download); it applies from a later task",
      failed: `RTK's Laya classifier could not start (%s); details in ${this.logPath}, and /rtk-laya-status retries it`,
    };
  }

  off() { return this.store.exists("laya-off"); }

  nodeCandidates() {
    const { platform, env, home } = this;
    const exe = platform === "win32" ? "node.exe" : "node";
    const dirs = (env.PATH || "").split(platform === "win32" ? ";" : ":").filter(Boolean);
    const nvm = (base) => {
      try { return fs.readdirSync(base).sort().reverse().map((v) => path.join(base, v, "bin")); } catch { return []; }
    };
    if (platform === "win32") {
      for (const d of [env.NVM_SYMLINK, env.ProgramFiles && path.join(env.ProgramFiles, "nodejs"), env["ProgramFiles(x86)"] && path.join(env["ProgramFiles(x86)"], "nodejs"), env.LOCALAPPDATA && path.join(env.LOCALAPPDATA, "Programs", "nodejs")]) if (d) dirs.push(d);
    } else {
      dirs.push("/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/snap/bin", path.join(home, ".volta", "bin"), path.join(home, ".local", "bin"), ...nvm(path.join(home, ".nvm", "versions", "node")));
    }
    return [...new Set(dirs)].map((d) => path.join(d, exe));
  }

  findNode() {
    if (this.env.RTK_DESKTOP_NODE) return this.env.RTK_DESKTOP_NODE === "none" ? null : this.env.RTK_DESKTOP_NODE;
    if (this._node && fs.existsSync(this._node)) return this._node;
    const major = (bin) => {
      try { return Number((/v(\d+)/.exec(spawnSync(bin, ["--version"], { encoding: "utf8", timeout: 5000 }).stdout || "") || [])[1] || 0); } catch { return 0; }
    };
    for (const c of this.nodeCandidates()) if (fs.existsSync(c) && major(c) >= 20) return (this._node = c);
    if (this.platform !== "win32") {
      try {
        const out = spawnSync("/bin/sh", ["-lc", "command -v node"], { encoding: "utf8", timeout: 10000 }).stdout.trim();
        if (out && major(out) >= 20) return (this._node = out);
      } catch {}
    }
    return null;
  }

  async up() {
    try {
      const r = await fetch(this.url + "/health", { signal: AbortSignal.timeout(1000) });
      return (await r.text()).trim() === "ok";
    } catch { return false; }
  }

  installed() { return fs.existsSync(path.join(this.dir, ".installed")); }
  firstRun() { return !this.installed() || !fs.existsSync(path.join(this.home, ".cache", "receptron-laya")); }
  installFailed() {
    try { return fs.readFileSync(path.join(this.dir, ".install-failed"), "utf8").trim(); } catch { return ""; }
  }

  npm(bin, args, timeout, env, out) {
    const c = npmCommand(this.platform, bin, args, env);
    return new Promise((resolve) => {
      let p;
      try { p = spawn(c.file, c.args, { cwd: this.dir, env, stdio: ["ignore", out, out], ...c.options }); } catch (e) { return resolve({ status: null, error: e.message }); }
      const timer = setTimeout(() => { try { p.kill(); } catch {} }, timeout);
      p.on("error", (e) => { clearTimeout(timer); resolve({ status: null, error: e.message }); });
      p.on("exit", (code, signal) => { clearTimeout(timer); resolve({ status: code, error: signal ? `npm ${signal}` : "" }); });
    });
  }

  async boot() {
    try {
      if (await this.up()) { this.phase = "up"; return; }
      const node = this.findNode();
      if (!node) throw new Error("Node.js 20+ not found on this computer");
      const bin = path.dirname(node);
      const env = { ...this.env, PATH: [bin, this.env.PATH || ""].join(this.platform === "win32" ? ";" : ":"), ONNXRUNTIME_NODE_INSTALL: "skip", LAYA_PORT: String(this.port), LAYA_EXIT_WITH_PARENT: "1" };
      fs.mkdirSync(this.dir, { recursive: true });
      if (this.src) fs.copyFileSync(this.src, path.join(this.dir, "server.mjs"));
      const out = fs.openSync(this.logPath, "a");
      try {
        if (!this.installed()) {
          this.phase = "installing @receptron/laya";
          if (!fs.existsSync(path.join(this.dir, "package.json"))) await this.npm(bin, ["init", "-y"], 60000, env, out);
          const r = await this.npm(bin, ["install", "--no-audit", "--no-fund", "@receptron/laya"], 900000, env, out);
          if (r.status !== 0) {
            const why = `npm install failed${r.error ? ` (${r.error})` : ""}, see laya.log`;
            fs.writeFileSync(path.join(this.dir, ".install-failed"), why + "\n");
            throw new Error(why);
          }
          fs.rmSync(path.join(this.dir, ".install-failed"), { force: true });
          fs.writeFileSync(path.join(this.dir, ".installed"), new Date().toISOString() + "\n");
        }
        this.phase = "loading model (first time downloads ~1.7 GB)";
        this.proc = spawn(node, ["server.mjs"], { cwd: this.dir, env, stdio: ["pipe", out, out] });
        this.proc.on("error", () => { this.proc = null; });
        this.proc.on("exit", () => { this.proc = null; if (this.phase === "up") this.phase = "stopped"; });
      } finally {
        fs.closeSync(out);
      }
      for (let i = 0; i < 720; i++) {
        if (await this.up()) { this.phase = "up"; this.store.log("laya up"); return; }
        if (!this.proc) throw new Error("laya exited (see laya.log)");
        await new Promise((r) => setTimeout(r, 2000));
      }
      throw new Error("laya not up after 24 minutes");
    } catch (e) {
      this.phase = "failed";
      this.error = e.message;
      this.store.log(`laya boot failed: ${e.message}`);
    }
  }

  start(retry = false) {
    if (this.phase !== "stopped" && this.phase !== "failed") return;
    const failed = this.installFailed();
    if (failed && !retry) { this.phase = "failed"; this.error = failed; return; }
    if (failed) fs.rmSync(path.join(this.dir, ".install-failed"), { force: true });
    this.phase = "starting";
    this.error = "";
    this.boot();
  }

  stop() {
    this.phase = "stopped";
    if (this.proc) { try { this.proc.kill(); } catch {} }
    this.proc = null;
  }

  watchIdle() {
    const t = setInterval(() => {
      if (this.proc && this.used && Date.now() - this.used > IDLE * 1000) { this.store.log(`laya idle ${IDLE}s, stopping`); this.stop(); }
    }, 60000);
    t.unref();
  }

  async waitUp(seconds) {
    for (let i = 0; i < seconds / 2; i++) {
      if (await this.up()) return true;
      if (this.phase === "failed") return false;
      await new Promise((r) => setTimeout(r, 2000));
    }
    return this.up();
  }

  async label(state, questions) {
    const t0 = Date.now();
    const r = await fetch(this.url + "/v1/systemone", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ state: state.slice(0, 2000), model: "laya", questions }),
      signal: AbortSignal.timeout(60000),
    });
    const answers = (await r.json()).answers || {};
    const labels = {};
    for (const [q, a] of Object.entries(answers)) if (a && a.choice) labels[q] = { choice: a.choice, confidence: Number(a.confidence || 0) };
    this.store.append("laya-verdicts.tsv", `${new Date().toISOString().slice(0, 19).replace("T", " ")}\t${JSON.stringify(labels)}\t${Date.now() - t0}ms`);
    return labels;
  }

  async status() {
    if (await this.up()) this.phase = "up";
    let n = 0;
    try { n = this.store.readText("laya-verdicts.tsv").split("\n").filter(Boolean).length; } catch {}
    const err = this.phase === "failed" ? ": " + this.error : "";
    return `${this.phase === "failed" ? "⚠️" : "✅"} Laya on your computer: **${this.phase}**${err} · task labelling ${this.off() ? "off" : "on"} · ${n} labels so far.`;
  }
}

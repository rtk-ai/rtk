import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const DEFAULTS = ["default-bash-compress", "explore-first"];

export function dataDir(platform = process.platform, env = process.env, home = os.homedir()) {
  if (platform === "darwin") return path.join(home, "Library", "Application Support", "rtk-desktop");
  if (platform === "win32") return path.join(env.APPDATA || path.join(home, "AppData", "Roaming"), "rtk-desktop");
  return path.join(env.XDG_DATA_HOME || path.join(home, ".local", "share"), "rtk-desktop");
}

export function stamp(d = new Date()) {
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export class Store {
  constructor(dir) {
    this.dir = dir;
    fs.mkdirSync(dir, { recursive: true });
  }
  path(...p) { return path.join(this.dir, ...p); }
  exists(...p) { return fs.existsSync(this.path(...p)); }
  log(msg) {
    try { fs.appendFileSync(this.path("server.log"), `${stamp()} ${msg}\n`); } catch {}
  }
  readJson(name, dflt) {
    try { return JSON.parse(fs.readFileSync(this.path(name), "utf8")); } catch { return dflt; }
  }
  writeJson(name, value) { fs.writeFileSync(this.path(name), JSON.stringify(value)); }
  readText(name) {
    try { return fs.readFileSync(this.path(name), "utf8"); } catch { return ""; }
  }
  append(name, line) {
    try { fs.appendFileSync(this.path(name), line + "\n"); } catch {}
  }
}

export function migrate(store, platform = process.platform, home = os.homedir()) {
  if (platform !== "darwin" || store.exists("migrated")) return;
  const old = path.join(home, "Library", "Application Support", "rtk-panel");
  if (!fs.existsSync(old)) return;
  const moved = [];
  try {
    for (const name of ["active.json", "laya-off", "laya-verdicts.tsv"]) {
      const src = path.join(old, name);
      if (fs.existsSync(src) && !store.exists(name)) { fs.copyFileSync(src, store.path(name)); moved.push(name); }
    }
    const pipes = path.join(old, "pipelines");
    if (fs.existsSync(pipes)) {
      fs.mkdirSync(store.path("pipelines"), { recursive: true });
      for (const f of fs.readdirSync(pipes)) if (f.endsWith(".toml") && !store.exists("pipelines", f)) { fs.copyFileSync(path.join(pipes, f), store.path("pipelines", f)); moved.push("pipelines/" + f); }
    }
    const laya = path.join(old, "laya");
    if (fs.existsSync(path.join(laya, ".installed")) && !store.exists("laya")) { fs.renameSync(laya, store.path("laya")); moved.push("laya/"); }
    fs.writeFileSync(store.path("migrated"), stamp() + "\n");
    store.log(`migrated from rtk-panel: ${moved.join(", ") || "nothing"}`);
  } catch (e) {
    store.log(`migration stopped: ${e.message}`);
  }
}

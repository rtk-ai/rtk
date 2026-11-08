import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

export const exeName = (platform) => (platform === "win32" ? "rtk.exe" : "rtk");

export function candidates(platform, env = process.env, home = os.homedir()) {
  const dirs = (env.PATH || "").split(platform === "win32" ? ";" : ":").filter(Boolean);
  if (platform === "win32") dirs.push(path.join(home, ".cargo", "bin"), path.join(env.LOCALAPPDATA || path.join(home, "AppData", "Local"), "Programs", "rtk"));
  else dirs.push(path.join(home, ".local", "bin"), path.join(home, ".cargo", "bin"), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin");
  return [...new Set(dirs)].map((d) => path.join(d, exeName(platform)));
}

export function isExe(p, platform = process.platform) {
  try { fs.accessSync(p, platform === "win32" ? fs.constants.F_OK : fs.constants.X_OK); return fs.statSync(p).isFile(); } catch { return false; }
}

export function versionOf(bin) {
  try {
    const r = spawnSync(bin, ["--version"], { encoding: "utf8", timeout: 5000 });
    const m = /(\d+)\.(\d+)\.(\d+)/.exec(r.stdout || "");
    return m ? m.slice(1).map(Number) : null;
  } catch { return null; }
}

export function findInstalled({ platform = process.platform, env = process.env, home = os.homedir(), login = true } = {}) {
  const found = [];
  if (login && platform !== "win32") {
    try { found.push(spawnSync("/bin/sh", ["-lc", "command -v rtk"], { encoding: "utf8", timeout: 10000 }).stdout.trim()); } catch {}
  }
  for (const c of [...found.filter(Boolean), ...candidates(platform, env, home)]) {
    if (!isExe(c, platform)) continue;
    const v = versionOf(c);
    if (v) return { path: c, version: v.join(".") };
  }
  return null;
}

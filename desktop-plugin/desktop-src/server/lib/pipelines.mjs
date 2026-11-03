import fs from "node:fs";
import path from "node:path";
import { DEFAULTS } from "./store.mjs";
import { get, pipelineUrl } from "./github.mjs";

export const ALLOWLIST = "rtk-ai/pipelines";
const TTL = 3600 * 1000;

function multiline(body, literal) {
  const s = body.replace(/^\r?\n/, "");
  if (literal) return s;
  let out = "";
  const joined = s.replace(/\\\r?\n[ \t\r\n]*/g, "");
  for (let i = 0; i < joined.length; i++) {
    const c = joined[i];
    if (c === "\\" && i + 1 < joined.length) out += c + joined[++i];
    else if (c === '"') out += '\\"';
    else if (c === "\n") out += "\\n";
    else if (c === "\r") out += "\\r";
    else if (c === "\t") out += "\\t";
    else out += c;
  }
  try { return JSON.parse(`"${out}"`); } catch { return joined; }
}

export function logicalLines(text) {
  const lines = [];
  const n = text.length;
  let cur = "", i = 0;
  while (i < n) {
    const c = text[i];
    if (c === "#") { while (i < n && text[i] !== "\n") i++; continue; }
    if (c === "\n") { lines.push(cur); cur = ""; i++; continue; }
    const q3 = text.startsWith('"""', i) ? '"""' : text.startsWith("'''", i) ? "'''" : null;
    if (q3) {
      const end = text.indexOf(q3, i + 3);
      cur += JSON.stringify(multiline(end < 0 ? text.slice(i + 3) : text.slice(i + 3, end), q3 === "'''"));
      i = end < 0 ? n : end + 3;
      continue;
    }
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && text[j] !== c && text[j] !== "\n") j += c === '"' && text[j] === "\\" ? 2 : 1;
      cur += text.slice(i, Math.min(j + 1, n));
      i = j + 1;
      continue;
    }
    cur += c;
    i++;
  }
  if (cur) lines.push(cur);
  return lines;
}

export const clean = (toml) => logicalLines(String(toml || "")).join("\n");

const unquote = (s) => s.trim().replace(/^"(.*)"$/, "$1").replace(/^'(.*)'$/, "$1");

export function parseToml(text) {
  const sections = {};
  let cur = null;
  for (const raw of logicalLines(text)) {
    const s = raw.trim();
    if (!s) continue;
    let m;
    if (s.startsWith("[[")) { cur = null; continue; }
    if ((m = /^\[(.+)\]$/.exec(s))) { cur = m[1].split(".").map(unquote).join("."); sections[cur] = sections[cur] || {}; continue; }
    if (!cur || !(m = /^("(?:[^"\\]|\\.)*"|'[^']*'|[\w.-]+)\s*=\s*(.*)$/.exec(s))) continue;
    const v = value(m[2]);
    if (v !== undefined) sections[cur][unquote(m[1])] = v;
  }
  return sections;
}

function closing(s) {
  let depth = 0, q = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) { if (c === "\\" && q === '"') i++; else if (c === q) q = null; continue; }
    if (c === '"' || c === "'") q = c;
    else if (c === "[") depth++;
    else if (c === "]" && --depth === 0) return i;
  }
  return s.length;
}

function splitTop(s) {
  const out = [];
  let depth = 0, q = null, cur = "";
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) { cur += c; if (c === "\\" && q === '"') cur += s[++i] ?? ""; else if (c === q) q = null; continue; }
    if (c === '"' || c === "'") q = c;
    else if (c === "[") depth++;
    else if (c === "]") depth--;
    else if (c === "," && depth === 0) { out.push(cur); cur = ""; continue; }
    cur += c;
  }
  if (cur.trim()) out.push(cur);
  return out;
}

function value(raw) {
  const s = raw.trim();
  let m;
  if ((m = /^"((?:[^"\\]|\\.)*)"/.exec(s))) { try { return JSON.parse('"' + m[1] + '"'); } catch { return m[1]; } }
  if ((m = /^'([^']*)'/.exec(s))) return m[1];
  if (s.startsWith("[")) return splitTop(s.slice(1, closing(s))).map(value).filter((x) => x !== undefined);
  if ((m = /^(true|false)\b/.exec(s))) return m[1] === "true";
  if ((m = /^[-+]?\d[\d_]*(\.\d[\d_]*)?([eE][-+]?\d+)?/.exec(s))) return Number(m[0].replace(/_/g, ""));
  return undefined;
}

export function pipelineName(toml) {
  const m = /^\[pipeline\]\s*$([^]*?)^\[/m.exec(clean(toml) + "\n[");
  const n = m && /^\s*name\s*=\s*"([\w.-]+)"/m.exec(m[1]);
  return n ? n[1] : null;
}

export const usesLaya = (toml) => /provider\s*=\s*"laya"/.test(clean(toml));
export const handsOff = (toml) => /action\s*=\s*"(deny|route)"/.test(clean(toml));

export function layaQuestions(toml) {
  const sec = parseToml(toml);
  const ups = new Set(Object.keys(sec).filter((n) => n.startsWith("upstream.") && sec[n].provider === "laya").map((n) => n.slice(9)));
  const q = {};
  for (const [n, d] of Object.entries(sec)) {
    if (!n.startsWith("classifier.") || n.split(".").length !== 2 || d.question !== "choice" || !ups.has(d.upstream)) continue;
    const labels = sec[n + ".labels"];
    if (labels && Object.keys(labels).length) q[n.split(".")[1]] = { type: "choice", instructions: d.instructions || "", criteria: labels };
  }
  return q;
}

export function pct(b, p) { return b ? Math.round((p / b - 1) * 100) : null; }

export const TABLE_HEAD = ["| Pipeline | Cost | Quality | Benchmark | Tier | By | Add |", "|---|---|---|---|---|---|---|"];
export function row(e, activeNames = []) {
  const name = `**${e.name}**` + (activeNames.includes(e.name) ? " ✅ in use" : DEFAULTS.includes(e.name) ? " (bundled default)" : "");
  let cost = "no benchmark published", bench = "";
  if (e.cost && e.cost.baseline && e.cost.pipeline) {
    const d = pct(e.cost.baseline, e.cost.pipeline);
    cost = `${d > 0 ? "🔴 +" : "🟢 "}${d}% vs ${e.baseline || "stock"}`;
    bench = [e.suite, e.model, e.trials && e.trials + " trials/arm"].filter(Boolean).join(", ");
  }
  const q = e.quality && e.quality.baseline != null && e.quality.pipeline != null ? `${e.quality.metric || "quality"} ${e.quality.baseline} → ${e.quality.pipeline}` : "";
  return `| ${name} | ${cost} | ${q} | ${bench} | ${e.tier || "?"} | ${e.owner} | \`/rtk-pipeline-add ${e.name}\` |`;
}

export class Pipelines {
  constructor(store, laya) {
    this.store = store;
    this.laya = laya;
    this.dir = store.path("pipelines");
  }

  active() {
    const names = this.store.readJson("active.json", null);
    return Array.isArray(names) ? names : [...DEFAULTS];
  }
  setActive(names) { this.store.writeJson("active.json", names); }

  async allowlist(force = false) {
    const cache = this.store.readJson("browse-cache.json", null);
    if (!force && cache && Date.now() - cache.at < TTL && Array.isArray(cache.entries)) return cache.entries;
    const tree = await get(`https://api.github.com/repos/${ALLOWLIST}/git/trees/main?recursive=1`, { json: true, max: 8_000_000 });
    const paths = (tree.tree || []).map((t) => t.path).filter((p) => /^approved\/[^/]+\/[^/]+\/[0-9a-f]{40}\/entry\.toml$/.test(p));
    const entries = [];
    for (const p of paths) {
      const base = `https://raw.githubusercontent.com/${ALLOWLIST}/main/${p.slice(0, -"entry.toml".length)}`;
      try {
        const entry = parseToml("[entry]\n" + (await get(base + "entry.toml"))).entry || {};
        const e = { owner: entry.owner, repo: entry.repo, sha: entry.sha, name: entry.name, tier: entry.tier, rtk: entry.rtk, submitted: entry.submitted_at };
        try {
          const b = parseToml(await get(base + "benchmark.toml"));
          const h = b.benchmark || {};
          Object.assign(e, { suite: h.suite, model: h.model, baseline: h.baseline, trials: h.trials_per_arm, cost: b["benchmark.cost"] || null, quality: b["benchmark.quality"] || null });
        } catch {}
        if (e.name && e.owner && e.repo && e.sha) entries.push(e);
      } catch (err) {
        this.store.log(`allowlist entry ${p}: ${err.message}`);
      }
    }
    const byName = new Map();
    for (const e of entries) if (!byName.has(e.name) || (e.submitted || "") > (byName.get(e.name).submitted || "")) byName.set(e.name, e);
    const out = [...byName.values()].sort((a, b) => a.name.localeCompare(b.name));
    this.store.writeJson("browse-cache.json", { at: Date.now(), entries: out });
    return out;
  }

  async fetchSource(source) {
    const src = String(source || "").trim();
    if (!src) throw new Error("say which pipeline: a name from /rtk-pipelines-search, github:owner/repo, a https:// URL or a file path");
    if (/^[\w.-]+$/.test(src)) {
      const e = (await this.allowlist()).find((x) => x.name === src);
      if (!e) throw new Error(`${src} is not on the reviewed list (/rtk-pipelines-search shows it); use github:owner/repo for an unreviewed pipeline`);
      return { toml: await get(`https://raw.githubusercontent.com/${e.owner}/${e.repo}/${e.sha}/pipeline.toml`), source: `github:${e.owner}/${e.repo}@${e.sha.slice(0, 7)}`, reviewed: true };
    }
    const gh = pipelineUrl(src);
    if (gh) return { toml: await get(gh), source: src };
    if (src.startsWith("https://")) return { toml: await get(src), source: src };
    if (/^([/~]|[A-Za-z]:[\\/])/.test(src)) return { toml: fs.readFileSync(src.replace(/^~/, process.env.HOME || ""), "utf8"), source: src };
    throw new Error("use a pipeline name, github:owner/repo, a https:// URL or a file path");
  }

  sources() { const s = this.store.readJson("sources.json", {}); return s && typeof s === "object" && !Array.isArray(s) ? s : {}; }

  async add(source) {
    const { toml, source: src, reviewed } = await this.fetchSource(source);
    const name = pipelineName(toml);
    if (!name) return "That file is not an rtk pipeline: it has no [pipeline] name.";
    fs.mkdirSync(this.dir, { recursive: true });
    fs.writeFileSync(`${this.dir}/${name}.toml`, toml);
    this.store.writeJson("sources.json", { ...this.sources(), [name]: { source: src, reviewed: !!reviewed } });
    const names = this.active();
    const notes = [];
    if (!names.includes(name)) names.push(name);
    if (handsOff(toml) && name !== "explore-first" && names.includes("explore-first")) {
      names.splice(names.indexOf("explore-first"), 1);
      notes.push("it replaces explore-first, since both hand work off");
    }
    this.setActive(names);
    if (usesLaya(toml)) notes.push("it uses Laya, which RTK starts on this computer" + (this.laya.findNode() ? "" : " once Node.js 20+ is installed"));
    if (/provider\s*=\s*"anthropic"/.test(clean(toml))) notes.push("rules that call a model directly stay silent without an API key");
    if (!reviewed) notes.push("nobody at RTK has reviewed this pipeline, so read it before you trust its rules");
    const rules = (clean(toml).match(/^\[\[hook\./gm) || []).length;
    return `✅ Added **${name}** from ${src} (${rules} rules). ${this.list()}${notes.length ? " Note: " + notes.join("; ") + "." : ""}`;
  }

  remove(name) {
    const names = this.active();
    if (!names.includes(name)) return `⚠️ ${name} is not in use. ${this.list()}`;
    names.splice(names.indexOf(name), 1);
    this.setActive(names);
    const sources = this.sources();
    if (name in sources) { delete sources[name]; this.store.writeJson("sources.json", sources); }
    return `✅ Removed **${name}**. ${this.list()}`;
  }

  reset() {
    this.setActive([...DEFAULTS]);
    return `✅ Back to the defaults: **${DEFAULTS.join("**, **")}**.`;
  }

  list() {
    const n = this.active(), sources = this.sources();
    const show = (x) => `**${x}**` + (!DEFAULTS.includes(x) && sources[x] && !sources[x].reviewed ? ` (unreviewed, from ${sources[x].source})` : "");
    return n.length ? `New tasks use: ${n.map(show).join(", ")}.` : "New tasks use no pipelines.";
  }

  async search(query) {
    const q = String(query || "").trim().toLowerCase();
    const all = await this.allowlist();
    const hits = q ? all.filter((e) => `${e.name} ${e.owner} ${e.tier}`.toLowerCase().includes(q)) : all;
    if (!hits.length) return q ? `Nothing on the reviewed list matches "${q}".` : "The reviewed list is empty.";
    const names = this.active();
    return [`**Reviewed RTK pipelines** (rtk-ai.app/pipelines), ${hits.length} of ${all.length}`, "", ...TABLE_HEAD, ...hits.map((e) => row(e, names))].join("\n");
  }

  async reviewedToml(e) {
    if (!/^[\w.-]+$/.test(e.name) || !/^[0-9a-f]{40}$/.test(e.sha)) throw new Error(`reviewed entry with an odd name or sha: ${e.name}`);
    const name = `${e.name}-${e.sha.slice(0, 7)}.toml`;
    const cached = this.store.readText(path.join("reviewed", name));
    if (cached) return cached;
    const toml = await get(`https://raw.githubusercontent.com/${e.owner}/${e.repo}/${e.sha}/pipeline.toml`);
    fs.mkdirSync(this.store.path("reviewed"), { recursive: true });
    fs.writeFileSync(this.store.path("reviewed", name), toml);
    return toml;
  }

  async choices(reviewed) {
    const active = this.active();
    const out = [{ name: "defaults", label: "Keep the defaults", inUse: DEFAULTS.every((n) => active.includes(n)) && active.length === DEFAULTS.length,
      text: "default-bash-compress + explore-first: compresses command output; after a few looks, searches go to a Haiku helper and implementations over about 150 lines to a Sonnet helper. Needs nothing." }];
    for (const e of reviewed) {
      if (DEFAULTS.includes(e.name)) continue;
      let what = [];
      try {
        const toml = clean(await this.reviewedToml(e));
        if (usesLaya(toml)) what.push("Laya, a classifier on your computer, labels each task");
        if (/haiku/i.test(toml)) what.push("searches go to a Haiku helper");
        if (/sonnet/i.test(toml)) what.push("implementations go to a Sonnet helper");
        if (/provider\s*=\s*"anthropic"/.test(toml)) what.push("some rules call a model directly (needs an API key)");

        if (usesLaya(toml)) what.push("needs Node.js 20+ for Laya");
      } catch (err) { this.store.log(`reviewed ${e.name}: ${err.message}`); }
      const bench = e.cost && e.cost.baseline && e.cost.pipeline ? `cost ${pct(e.cost.baseline, e.cost.pipeline)}% vs ${e.baseline || "stock"}` + (e.quality && e.quality.baseline != null ? `, ${e.quality.metric || "quality"} ${e.quality.baseline} → ${e.quality.pipeline}` : "") + ` (${[e.suite, e.model, e.trials && e.trials + " trials/arm"].filter(Boolean).join(", ")})` : "no benchmark published";
      out.push({ name: e.name, label: `Add ${e.name}`, inUse: active.includes(e.name), text: `${bench}. ${what.length ? what.join("; ") + "." : ""}`.trim() });
    }
    return out;
  }

  deliverable(names) {
    const pipelines = {};
    let questions = {}, laya = false;
    for (const name of names) {
      const p = `${this.dir}/${name}.toml`;
      if (DEFAULTS.includes(name) && !fs.existsSync(p)) continue;
      let toml;
      try { toml = fs.readFileSync(p, "utf8"); } catch { continue; }
      pipelines[name] = toml;
      laya = laya || usesLaya(toml);
      questions = { ...questions, ...layaQuestions(toml) };
    }
    return { pipelines, questions, usesLaya: laya };
  }
}

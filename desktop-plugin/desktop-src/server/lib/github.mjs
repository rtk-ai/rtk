export const UA = "rtk-desktop";
const HOPS = 5;

export function allowedUrl(u) {
  return u.protocol === "https:" || (u.protocol === "http:" && /^(127\.0\.0\.1|localhost|\[::1\])$/.test(u.hostname));
}

export async function get(url, { timeout = 30000, max = 2_000_000, json = false, binary = false } = {}) {
  let u = new URL(url);
  const signal = AbortSignal.timeout(timeout);
  let r;
  for (let hop = 0; ; hop++) {
    if (!allowedUrl(u)) throw new Error(`refused ${u.protocol}//${u.host} (https only): ${url}`);
    r = await fetch(u, { headers: { "user-agent": UA, accept: json ? "application/vnd.github+json" : "*/*" }, signal, redirect: "manual" });
    const to = r.status >= 300 && r.status < 400 && r.headers.get("location");
    if (!to) break;
    if (hop >= HOPS) throw new Error(`too many redirects: ${url}`);
    try { await r.body?.cancel(); } catch {}
    u = new URL(to, u);
  }
  if (!r.ok) throw new Error(`HTTP ${r.status} for ${url}`);
  if (Number(r.headers.get("content-length") || 0) > max) throw new Error(`too large (${r.headers.get("content-length")} bytes): ${url}`);
  const chunks = [];
  let size = 0;
  for await (const chunk of r.body) {
    size += chunk.length;
    if (size > max) throw new Error(`too large (over ${max} bytes): ${url}`);
    chunks.push(chunk);
  }
  const buf = Buffer.concat(chunks);
  if (binary) return buf;
  const text = buf.toString("utf8");
  return json ? JSON.parse(text) : text;
}

export function pipelineUrl(source) {
  const m = /^(?:github:|https:\/\/github\.com\/)([\w.-]+)\/([\w.-]+?)(?:\.git)?(?:@([\w./-]+))?\/?$/.exec(source.trim());
  if (!m || [m[1], m[2]].some((p) => /^\.+$/.test(p)) || /(^|\/)\.\.?(\/|$)/.test(m[3] || "")) return null;
  return `https://raw.githubusercontent.com/${m[1]}/${m[2]}/${m[3] || "HEAD"}/pipeline.toml`;
}

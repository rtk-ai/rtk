export const UA = "rtk-desktop";

export async function get(url, { timeout = 30000, max = 2_000_000, json = false, binary = false } = {}) {
  const r = await fetch(url, {
    headers: { "user-agent": UA, accept: json ? "application/vnd.github+json" : "*/*" },
    signal: AbortSignal.timeout(timeout),
    redirect: "follow",
  });
  if (!r.ok) throw new Error(`HTTP ${r.status} for ${url}`);
  const buf = Buffer.from(await r.arrayBuffer());
  if (buf.length > max) throw new Error(`too large (${buf.length} bytes): ${url}`);
  if (binary) return buf;
  const text = buf.toString("utf8");
  return json ? JSON.parse(text) : text;
}

export function pipelineUrl(source) {
  const m = /^(?:github:|https:\/\/github\.com\/)([\w.-]+)\/([\w.-]+?)(?:\.git)?(?:@([\w./-]+))?\/?$/.exec(source.trim());
  if (!m) return null;
  return `https://raw.githubusercontent.com/${m[1]}/${m[2]}/${m[3] || "HEAD"}/pipeline.toml`;
}

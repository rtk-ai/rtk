import { createServer } from "node:http";
import { Laya } from "@receptron/laya";

const PORT = Number(process.env.LAYA_PORT ?? 41733);
const laya = await Laya.load();
console.log("laya: model warm");

const server = createServer((req, res) => {
  if (req.method === "GET" && req.url === "/health") return res.end("ok");
  if (req.method !== "POST" || !req.url.startsWith("/v1/systemone")) { res.statusCode = 404; return res.end(); }
  let body = "";
  req.setEncoding("utf8");
  req.on("data", (c) => { body += c; if (body.length > 1_000_000) req.destroy(); });
  req.on("end", async () => {
    try {
      const { state, questions } = JSON.parse(body);
      const t0 = Date.now();
      const result = await laya.systemOne(state ?? "", questions ?? {});
      const answers = {};
      for (const [name, a] of Object.entries(result.answers)) {
        if (a.type === "score") {
          const levels = Object.keys(a.probabilities ?? {}).length;
          answers[name] = { ...a, score: levels > 1 ? a.score / (levels - 1) : a.score };
        } else answers[name] = a;
      }
      console.log(`laya: verdict ${Date.now() - t0}ms chars=${(state ?? "").length} ${JSON.stringify(Object.fromEntries(Object.entries(answers).map(([k, a]) => [k, a.noul ?? a.score ?? a.choice])))}`);
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify({ model: result.model, answers, usage: result.usage }));
    } catch (e) {
      console.error("laya: request failed", e);
      res.statusCode = 400;
      res.end(JSON.stringify({ error: "Bad request" }));
    }
  });
});
const HOST = process.env.LAYA_HOST ?? "127.0.0.1";
server.listen(PORT, HOST, () => console.log(`laya: listening on ${HOST}:${PORT}/v1/systemone`));
if (process.env.LAYA_EXIT_WITH_PARENT === "1") {
  process.stdin.on("end", () => { console.log("laya: parent gone, exiting"); process.exit(0); });
  process.stdin.on("error", () => process.exit(0));
  process.stdin.resume();
}


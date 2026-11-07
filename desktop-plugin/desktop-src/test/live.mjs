import fs from "node:fs";
import { Store } from "../server/lib/store.mjs";
import { Laya } from "../server/lib/laya.mjs";
import { Pipelines } from "../server/lib/pipelines.mjs";
import { findInstalled } from "../server/lib/rtklocal.mjs";
import { localGain, gainText } from "../server/lib/ledger.mjs";

const dir = process.argv[2];
if (!dir) { console.error("usage: node test/live.mjs <store dir>"); process.exit(2); }
const store = new Store(dir);
const rtk = findInstalled();
console.log("installed rtk:", rtk);
console.log(gainText([], localGain(rtk && rtk.path), rtk));
const pipes = new Pipelines(store, new Laya(store, { env: { ...process.env, RTK_DESKTOP_NODE: "none" } }));
const t1 = Date.now();
console.log(await pipes.search(""), `(${Date.now() - t1}ms)`);
console.log(await pipes.add("laya-tiered"));
console.log(pipes.list());
console.log("files:", fs.readdirSync(store.path("pipelines")).join(", "));
try { await pipes.add("not-a-pipeline"); } catch (e) { console.log("expected error:", e.message); }

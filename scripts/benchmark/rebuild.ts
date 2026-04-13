#!/usr/bin/env bun
/**
 * Fast rebuild: reuse existing VM, just transfer source and recompile.
 * Usage: bun run scripts/benchmark/rebuild.ts
 */

import { vmEnsureReady, vmBuildRtk } from "./lib/vm";

const PROJECT_ROOT = new URL("../../", import.meta.url).pathname.replace(/\/$/, "");

await vmEnsureReady();
const info = await vmBuildRtk(PROJECT_ROOT);

console.log(`\nRebuild complete:`);
console.log(`  Version: ${info.version}`);
console.log(`  Binary:  ${info.binarySize} bytes`);
console.log(`  Time:    ${info.buildTime}s`);

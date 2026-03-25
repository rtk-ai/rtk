#!/usr/bin/env bun
/**
 * Delete the RTK test VM.
 * Usage: bun run scripts/benchmark/cleanup.ts
 */

import { vmDelete } from "./lib/vm";

console.log("Deleting rtk-test VM...");
await vmDelete();
console.log("Done.");

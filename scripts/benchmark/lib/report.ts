/**
 * Report generation for RTK integration test results.
 */

import type { TestResult } from "./test";
import { getCounts, getResults } from "./test";

interface BuildInfo {
  buildTime: number;
  binarySize: number;
  version: string;
  branch: string;
  commit: string;
}

export function generateReport(buildInfo: BuildInfo): string {
  const { total, passed, failed, skipped } = getCounts();
  const results = getResults();
  const passRate = total > 0 ? Math.round((passed * 100) / total) : 0;

  const lines: string[] = [];

  lines.push("======================================================");
  lines.push("        RTK INTEGRATION TEST REPORT");
  lines.push("======================================================");
  lines.push("");
  lines.push(`Date:    ${new Date().toISOString()}`);
  lines.push(`Branch:  ${buildInfo.branch}`);
  lines.push(`Commit:  ${buildInfo.commit}`);
  lines.push(`Version: ${buildInfo.version}`);
  lines.push(`Binary:  ${buildInfo.binarySize} bytes`);
  lines.push(`Build:   ${buildInfo.buildTime}s`);
  lines.push("");

  // Summary
  lines.push("--- Summary ---");
  lines.push(`Total:   ${total}`);
  lines.push(`Passed:  ${passed} (${passRate}%)`);
  lines.push(`Failed:  ${failed}`);
  lines.push(`Skipped: ${skipped}`);
  lines.push("");

  // Group results by phase (name prefix before ":")
  const phases = new Map<string, TestResult[]>();
  for (const r of results) {
    const colonIdx = r.name.indexOf(":");
    const phase = colonIdx > 0 ? r.name.slice(0, colonIdx) : "misc";
    if (!phases.has(phase)) phases.set(phase, []);
    phases.get(phase)!.push(r);
  }

  for (const [phase, phaseResults] of phases) {
    const pPassed = phaseResults.filter((r) => r.status === "PASS").length;
    const pTotal = phaseResults.length;
    lines.push(`--- ${phase} (${pPassed}/${pTotal}) ---`);

    for (const r of phaseResults) {
      const shortName = r.name.includes(":") ? r.name.split(":")[1] : r.name;
      lines.push(`  ${r.status.padEnd(4)} | ${shortName} | ${r.detail}`);
    }
    lines.push("");
  }

  // Failures detail
  const failures = results.filter((r) => r.status === "FAIL");
  if (failures.length > 0) {
    lines.push("--- Failures ---");
    for (const f of failures) {
      lines.push(`  ${f.name}: ${f.detail}`);
    }
    lines.push("");
  }

  // Token savings summary
  const savingsResults = results.filter((r) => r.savings !== undefined);
  if (savingsResults.length > 0) {
    const avgSavings = Math.round(
      savingsResults.reduce((sum, r) => sum + (r.savings ?? 0), 0) /
        savingsResults.length
    );
    const minSavings = Math.min(
      ...savingsResults.map((r) => r.savings ?? 100)
    );
    const maxSavings = Math.max(...savingsResults.map((r) => r.savings ?? 0));
    lines.push("--- Token Savings ---");
    lines.push(`Average: ${avgSavings}%`);
    lines.push(`Min:     ${minSavings}%`);
    lines.push(`Max:     ${maxSavings}%`);
    lines.push("");
  }

  // Verdict
  lines.push("======================================================");
  if (failed === 0) {
    lines.push("  Verdict: READY FOR RELEASE");
  } else {
    lines.push(`  Verdict: NOT READY (${failed} failures)`);
  }
  lines.push("======================================================");

  return lines.join("\n");
}

/** Save report to file */
export async function saveReport(
  buildInfo: BuildInfo,
  outPath: string
): Promise<string> {
  const report = generateReport(buildInfo);
  await Bun.write(outPath, report);
  console.log(`\nReport saved to: ${outPath}`);
  return report;
}

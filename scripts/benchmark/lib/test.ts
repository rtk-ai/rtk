/**
 * Test helpers for RTK integration testing.
 */

import { vmExec, RTK_BIN } from "./vm";

export type TestStatus = "PASS" | "FAIL" | "SKIP";

export interface TestResult {
  name: string;
  status: TestStatus;
  detail: string;
  exitCode?: number;
  outputSize?: number;
  savings?: number;
  duration?: number;
}

const results: TestResult[] = [];

export function getResults(): TestResult[] {
  return results;
}

export function getCounts() {
  const total = results.length;
  const passed = results.filter((r) => r.status === "PASS").length;
  const failed = results.filter((r) => r.status === "FAIL").length;
  const skipped = results.filter((r) => r.status === "SKIP").length;
  return { total, passed, failed, skipped };
}

function record(result: TestResult) {
  results.push(result);
  const icon =
    result.status === "PASS"
      ? "\x1b[32mPASS\x1b[0m"
      : result.status === "FAIL"
        ? "\x1b[31mFAIL\x1b[0m"
        : "\x1b[33mSKIP\x1b[0m";
  console.log(`  ${icon} | ${result.name} | ${result.detail}`);
}

/**
 * Test a command exits with expected code and doesn't crash.
 * expectedExit: number or "any" (just checks no signal death)
 */
export async function testCmd(
  name: string,
  cmd: string,
  expectedExit: number | "any" = 0
): Promise<TestResult> {
  const start = Date.now();
  const { stdout, stderr, exitCode } = await vmExec(cmd);
  const duration = Date.now() - start;
  const outputSize = stdout.length + stderr.length;

  let status: TestStatus;
  let detail: string;

  if (expectedExit === "any") {
    // Just check it didn't die from signal (exit >= 128)
    if (exitCode < 128) {
      status = "PASS";
      detail = `exit=${exitCode} | ${outputSize}b | ${duration}ms`;
    } else {
      status = "FAIL";
      detail = `SIGNAL exit=${exitCode} | ${outputSize}b`;
    }
  } else if (exitCode === expectedExit) {
    status = "PASS";
    detail = `exit=${exitCode} | ${outputSize}b | ${duration}ms`;
  } else {
    status = "FAIL";
    detail = `expected exit=${expectedExit}, got ${exitCode} | ${outputSize}b`;
  }

  const result: TestResult = {
    name,
    status,
    detail,
    exitCode,
    outputSize,
    duration,
  };
  record(result);
  return result;
}

/**
 * Test token savings: compare raw command output vs RTK filtered output.
 */
export async function testSavings(
  name: string,
  rawCmd: string,
  rtkCmd: string,
  targetPct: number
): Promise<TestResult> {
  const raw = await vmExec(rawCmd);
  const rtk = await vmExec(rtkCmd);

  const rawSize = raw.stdout.length;
  const rtkSize = rtk.stdout.length;

  if (rawSize === 0) {
    const result: TestResult = {
      name,
      status: "SKIP",
      detail: "raw output empty",
    };
    record(result);
    return result;
  }

  const savings = Math.round(100 - (rtkSize * 100) / rawSize);

  let status: TestStatus;
  let detail: string;

  if (savings >= targetPct) {
    status = "PASS";
    detail = `raw=${rawSize}b filtered=${rtkSize}b savings=${savings}% (target: >=${targetPct}%)`;
  } else {
    status = "FAIL";
    detail = `savings=${savings}% < target ${targetPct}% (raw=${rawSize}b filtered=${rtkSize}b)`;
  }

  const result: TestResult = { name, status, detail, savings };
  record(result);
  return result;
}

/**
 * Test rewrite engine: input -> expected output.
 */
export async function testRewrite(
  input: string,
  expected: string
): Promise<TestResult> {
  const { stdout } = await vmExec(`${RTK_BIN} rewrite '${input}'`);
  const actual = stdout.trim();

  let status: TestStatus;
  let detail: string;

  if (actual === expected) {
    status = "PASS";
    detail = `'${input}' -> '${actual}'`;
  } else {
    status = "FAIL";
    detail = `'${input}' -> expected '${expected}', got '${actual}'`;
  }

  const result: TestResult = { name: `rewrite: ${input}`, status, detail };
  record(result);
  return result;
}

/**
 * Skip a test with a reason.
 */
export function skipTest(name: string, reason: string): TestResult {
  const result: TestResult = { name, status: "SKIP", detail: reason };
  record(result);
  return result;
}

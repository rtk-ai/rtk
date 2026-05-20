/**
 * Behavioral tests for the OMP RTK hook.
 *
 * Mirrors the Hermes plugin test suite (hooks/hermes/tests/test_rtk_rewrite_plugin.py).
 * Run with: bun test hooks/omp/tests/
 */

import { describe, test, expect, mock, beforeAll, beforeEach, afterEach, spyOn } from "bun:test";

// ── Mocks for external packages (not installed in dev) ─────────────

mock.module("@oh-my-pi/pi-utils", () => ({
  $which: () => "/usr/bin/rtk",
}));

mock.module("@oh-my-pi/pi-coding-agent/extensibility/hooks", () => ({}));

// ── Fake subprocess ───────────────────────────────────────────────

function fakeSubprocess(exitCode: number, stdout: string, stderr: string) {
  const encoder = new TextEncoder();
  return {
    exited: Promise.resolve(exitCode),
    stdout: new ReadableStream({
      start(controller: ReadableStreamDefaultController<Uint8Array>) {
        controller.enqueue(encoder.encode(stdout));
        controller.close();
      },
    }),
    stderr: new ReadableStream({
      start(controller: ReadableStreamDefaultController<Uint8Array>) {
        controller.enqueue(encoder.encode(stderr));
        controller.close();
      },
    }),
    kill: mock(() => {}),
  };
}

// ── Test suite ────────────────────────────────────────────────────

describe("OMP RTK hook", () => {
  let mod: typeof import("../rtk.ts");
  let consoleErrorSpy: ReturnType<typeof spyOn>;
  let origSpawn: (...args: any[]) => any;

  beforeAll(async () => {
    mod = await import("../rtk.ts");
    origSpawn = mod.spawner.spawn;
  });

  beforeEach(() => {
    consoleErrorSpy = spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    mod.spawner.spawn = origSpawn;
    consoleErrorSpy.mockRestore();
  });

  // ── Helpers ─────────────────────────────────────────────────

  function setSpawn(exitCode: number, stdout: string, stderr: string) {
    mod.spawner.spawn = () => fakeSubprocess(exitCode, stdout, stderr);
  }

  function setSpawnError(message: string) {
    mod.spawner.spawn = () => {
      throw new Error(message);
    };
  }

  // ── Registration ────────────────────────────────────────────

  test("registers tool_call handler", () => {
    const listeners = new Map<string, (...args: any[]) => void>();
    const api = {
      on(event: string, handler: (...args: any[]) => void) {
        listeners.set(event, handler);
      },
    };
    mod.default(api);

    expect(listeners.has("tool_call")).toBe(true);
  });

  // ── Rewrite success ─────────────────────────────────────────

  test("rewrite success (exit 0) returns allow verdict", async () => {
    setSpawn(0, "rtk git status\n", "");

    const result = await mod.rewrite("git status");

    expect(result).not.toBeNull();
    expect(result!.rewritten).toBe("rtk git status");
    expect(result!.verdict).toBe("allow");
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  test("rewrite exit code 3 returns ask verdict", async () => {
    setSpawn(3, "rtk git status\n", "");

    const result = await mod.rewrite("git status");

    expect(result).not.toBeNull();
    expect(result!.rewritten).toBe("rtk git status");
    expect(result!.verdict).toBe("ask");
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  test("rewrite exit 0 with unchanged output returns null", async () => {
    setSpawn(0, "git status\n", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  // ── Passthrough exit codes ──────────────────────────────────

  test("exit code 1 (no rtk equivalent) returns null silently", async () => {
    setSpawn(1, "", "unused stderr");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  test("exit code 2 (deny rule) returns null silently", async () => {
    setSpawn(2, "", "unused stderr");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  // ── Error handling ──────────────────────────────────────────

  test("unexpected exit code warns with stderr detail", async () => {
    setSpawn(4, "rtk git status\n", "bad news");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      "rtk: omp hook warning: rtk rewrite failed with exit 4: bad news",
    );
  });

  test("unexpected exit code without stderr warns with code only", async () => {
    setSpawn(5, "rtk git status\n", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      "rtk: omp hook warning: rtk rewrite failed with exit 5",
    );
  });

  test("spawn error warns and returns null", async () => {
    setSpawnError("spawn ENOENT");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).toHaveBeenCalledWith(
      "rtk: omp hook warning: rtk rewrite error: spawn ENOENT",
    );
  });

  // ── Empty / unchanged output ────────────────────────────────

  test("empty stdout returns null for exit 0", async () => {
    setSpawn(0, "", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
  });

  test("whitespace-only stdout returns null for exit 0", async () => {
    setSpawn(0, "\n", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
  });

  test("exit 3 with empty stdout returns null without warning", async () => {
    setSpawn(3, "", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
    expect(consoleErrorSpy).not.toHaveBeenCalled();
  });

  test("exit 3 with unchanged stdout returns null", async () => {
    setSpawn(3, "git status\n", "");

    const result = await mod.rewrite("git status");

    expect(result).toBeNull();
  });
});

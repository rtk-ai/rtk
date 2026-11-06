// RTK Pi extension — rewrites bash commands to use rtk for token savings.
// Shared with Oh My Pi (OMP) — OMP loads this same file via its legacy-pi-compat layer.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating extension: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// Exit code contract for `rtk rewrite`:
//   0 + stdout  Rewrite found → mutate command
//   1           No RTK equivalent → pass through unchanged
//   3 + stdout  Rewrite (advisory) → mutate command

import type {
  BashToolCallEvent,
  ExtensionAPI,
  ToolCallEvent,
} from "@earendil-works/pi-coding-agent"

const REWRITE_TIMEOUT_MS = 2_000
const MIN_SUPPORTED_RTK_MINOR = 23
const MAX_RECOVERY_RETRIES = 3
const RECOVERY_COOLDOWN_MS = 10_000
const RTK_NOT_FOUND_REASON = "rtk binary not found in PATH"
const RTK_EXHAUSTED_REASON =
  "rtk binary not found in PATH — check PATH and restart or reload the extension"

// Local reimplementation of the package's `isToolCallEventType("bash", event)` type
// guard. That helper is a value export, so importing it pulls in the whole
// `@earendil-works/pi-coding-agent` barrel at extension load — profiled at ~250ms
// warmed, vs ~10ms for a type-only import. `BashToolCallEvent`/`ToolCallEvent`
// below are type-only imports and are erased at compile time, so they carry none
// of that cost. See #2753.
function isBashToolCallEvent(event: ToolCallEvent): event is BashToolCallEvent {
  return event.toolName === "bash"
}

// Parse "X.Y.Z" semver, return [major, minor, patch] or null.
function parseSemver(raw: string): [number, number, number] | null {
  const m = raw.trim().match(/(\d+)\.(\d+)\.(\d+)/)
  if (!m) return null
  return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)]
}

type RewriteCheckOutcome =
  | { ok: true; rewritten: string | null }
  | { ok: false }

// Calls `rtk rewrite`; distinguishes a valid check (exit 0, 1, or 3) from a
// failed invocation so degraded-mode recovery is only confirmed on a valid run.
async function runRewriteCheck(
  pi: ExtensionAPI,
  cmd: string,
  signal?: AbortSignal
): Promise<RewriteCheckOutcome> {
  const result = await pi.exec("rtk", ["rewrite", cmd], {
    timeout: REWRITE_TIMEOUT_MS,
    signal,
  })
  if (result.killed) return { ok: false }
  if (result.code === 1) return { ok: true, rewritten: null }
  if (result.code === 0 || result.code === 3) {
    return { ok: true, rewritten: result.stdout.trim() || null }
  }
  return { ok: false }
}

type VersionProbeResult =
  | { status: "supported" }
  | { status: "unsupported"; version: string; rawVersion: string }
  | { status: "unavailable" }

async function probeRtkVersion(
  pi: ExtensionAPI,
  signal?: AbortSignal
): Promise<VersionProbeResult> {
  try {
    const ver = await pi.exec("rtk", ["--version"], {
      timeout: REWRITE_TIMEOUT_MS,
      signal,
    })
    if (ver.killed || ver.code !== 0) {
      return { status: "unavailable" }
    }
    const parsed = parseSemver((ver.stdout ?? "").replace(/^rtk\s+/, ""))
    if (parsed) {
      const [major, minor] = parsed
      if (major === 0 && minor < MIN_SUPPORTED_RTK_MINOR) {
        return {
          status: "unsupported",
          version: parsed.join("."),
          rawVersion: ver.stdout.trim(),
        }
      }
    }
    return { status: "supported" }
  } catch {
    return { status: "unavailable" }
  }
}

type StatusContext = {
  ui?: {
    setStatus?: (key: string, text: string | undefined) => void
  }
}

// Register before the async version probe so a host cannot miss the handler
// while the probe is in flight. If session_start happens first, retain its
// context and apply the status as soon as the probe reports a failure.
// pi.notify is intentionally not used — OMP wipes it on the initial render.
function registerRtkUnavailableNotice(pi: ExtensionAPI) {
  let reason: string | undefined
  let hasReportedStatus = false
  let sessionContext: StatusContext | undefined

  const applyToContext = (
    target: StatusContext | undefined,
    text: string | undefined
  ) => {
    if (!target) return
    try {
      target.ui?.setStatus?.("rtk", text)
    } catch {
      // Status reporting must never affect the extension's fail-open behavior.
    }
  }

  const applyStatus = (extraContext?: unknown) => {
    const extra = extraContext as StatusContext | undefined
    if (extra?.ui?.setStatus && !sessionContext) {
      sessionContext = extra
    }
    if (reason) {
      const text = `RTK disabled: ${reason}`
      hasReportedStatus = true
      applyToContext(sessionContext, text)
      if (extra && extra !== sessionContext) {
        applyToContext(extra, text)
      }
    } else if (hasReportedStatus) {
      hasReportedStatus = false
      applyToContext(sessionContext, undefined)
      if (extra && extra !== sessionContext) {
        applyToContext(extra, undefined)
      }
    }
  }

  try {
    pi.on("session_start", (_event: unknown, ctx: unknown) => {
      sessionContext = ctx as StatusContext
      applyStatus()
    })
  } catch {
    // Runtimes without a session_start event: fall back to tool_call context if available.
  }

  return {
    report(nextReason: string, ctx?: unknown) {
      reason = nextReason
      applyStatus(ctx)
    },
    clear(ctx?: unknown) {
      reason = undefined
      applyStatus(ctx)
    },
  }
}

type ExtensionState = "ready" | "degraded" | "exhausted" | "unsupported"

export default async function (pi: ExtensionAPI) {
  const statusNotice = registerRtkUnavailableNotice(pi)

  // Probe rtk version at load time; disables permanently if too old, or enters
  // bounded degraded recovery if the startup probe fails.
  const initialProbe = await probeRtkVersion(pi)
  if (initialProbe.status === "unsupported") {
    statusNotice.report(`rtk ${initialProbe.version} is too old (need >= 0.23.0)`)
    console.warn(
      `[rtk] rtk ${initialProbe.rawVersion} is too old (need >= 0.23.0) — extension disabled`
    )
    return
  }

  let state: ExtensionState = "ready"
  let retriesUsed = 0
  let lastRetryAt: number | undefined

  if (initialProbe.status === "unavailable") {
    state = "degraded"
    statusNotice.report(RTK_NOT_FOUND_REASON)
    console.warn(`[rtk] ${RTK_NOT_FOUND_REASON} — extension degraded; will retry`)
  }

  const markExhaustedIfNeeded = (ctx: unknown) => {
    if (retriesUsed >= MAX_RECOVERY_RETRIES) {
      state = "exhausted"
      statusNotice.report(RTK_EXHAUSTED_REASON, ctx)
      console.warn(`[rtk] ${RTK_EXHAUSTED_REASON}`)
    }
  }

  pi.on("tool_call", async (event, ctx) => {
    try {
      if (!isBashToolCallEvent(event)) return

      const cmd = event.input.command
      if (typeof cmd !== "string" || cmd.trim() === "") return

      if (cmd.startsWith("rtk ")) return
      if (process.env.RTK_DISABLED === "1") return

      if (state === "exhausted" || state === "unsupported") return

      if (state === "degraded") {
        if (retriesUsed >= MAX_RECOVERY_RETRIES) return
        const now = Date.now()
        if (
          lastRetryAt !== undefined &&
          now - lastRetryAt < RECOVERY_COOLDOWN_MS
        ) {
          return
        }

        retriesUsed += 1
        lastRetryAt = now

        const probe = await probeRtkVersion(pi, ctx?.signal)
        if (probe.status === "unsupported") {
          state = "unsupported"
          statusNotice.report(
            `rtk ${probe.version} is too old (need >= 0.23.0)`,
            ctx
          )
          console.warn(
            `[rtk] rtk ${probe.rawVersion} is too old (need >= 0.23.0) — extension disabled`
          )
          return
        }
        if (probe.status === "unavailable") {
          markExhaustedIfNeeded(ctx)
          return
        }

        let check: RewriteCheckOutcome
        try {
          check = await runRewriteCheck(pi, cmd, ctx?.signal)
        } catch {
          check = { ok: false }
        }
        if (!check.ok) {
          markExhaustedIfNeeded(ctx)
          return
        }

        state = "ready"
        retriesUsed = 0
        lastRetryAt = undefined
        statusNotice.clear(ctx)

        if (check.rewritten && check.rewritten !== cmd) {
          event.input.command = check.rewritten
        }
        return
      }

      // Delegate to RTK.
      const check = await runRewriteCheck(pi, cmd, ctx?.signal)
      if (check.ok && check.rewritten && check.rewritten !== cmd) {
        event.input.command = check.rewritten
      }
    } catch (err) {
      // Fail open: never block execution on an unexpected error.
      console.warn(
        "[rtk] unexpected error in tool_call handler; passing through command",
        err
      )
      return
    }
  })
}

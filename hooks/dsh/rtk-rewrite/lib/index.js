// RTK cordis plugin for DeepSeek Harness (DSH) — rewrites shell commands to
// their RTK equivalents before execution, so DSH's bash/pwsh tool receives
// compact command output without changing the model's or the user's workflow.
//
// Why this plugin exists: DSH's `tools/pre-execute` event (see
// `@deepseek-ai/dsh-tools`) is deliberately allow/deny-only — it cannot
// rewrite `exec.arguments`. The only seam that can change the command text
// before it runs is `ctx.shell`, a cordis service every bash/pwsh tool call
// and background job resolves through (`dsh-tool-bash`'s `execute`/`resolve`
// calls). This plugin patches that service's `execute` method so the command
// text is swapped for RTK's rewritten form immediately before it reaches the
// real executor (`dsh-bash-local` / `dsh-pwsh-local`).
//
// Important limitation (document this for users, do not paper over it): this
// is a post-hoc substitution, not a `pre-tool-input-rewrite` hook. DSH's own
// session history, audit log, and UI card still show the command exactly as
// the model wrote it; only the process that actually spawns runs the RTK
// form. A persistent terminal (pty) session does not go through `ctx.shell`
// at all, so commands typed there are never rewritten.
//
// All rewrite *decisions* live in RTK's Rust binary (`rtk rewrite`), the same
// single source of truth every other delegate (Hermes, Pi, OMP) calls — see
// `src/hooks/rewrite_cmd.rs` for the exit-code contract this plugin consumes.
// This file only carries the transport and three safety nets:
//
//   1. Fail open — any missing binary, bad exit code, timeout, or unexpected
//      exception falls back to running the original command unchanged. This
//      plugin must never be the reason a command does not run.
//   2. Whitelist — a rewrite is only accepted when every `rtk <name>` prefix
//      it inserts names a subcommand this plugin recognizes as a safe RTK
//      wrapper (`DEFAULT_COMMANDS`, overridable via `config.commands`).
//      Deliberately excludes shell builtins/aliases whose semantics change
//      across hosts (PowerShell's `ls` → `Get-ChildItem` returns an object
//      stream, not text; converting it to `rtk ls` would change what the
//      command returns).
//   3. Bypass switch — a sentinel file
//      (`~/.dsh/rtk/off`, shared with other RTK host integrations) disables
//      this plugin globally without uninstalling it.
//
// Token-level guard (the fourth net, see `isSafeRewrite` below): RTK's
// rewrite is only ever accepted when it is provably "the original command,
// with one or more extra literal `rtk` tokens inserted" — tokenized on shell
// operators (`&& || | ; &`), never on anything else. Any other difference
// (a changed, added, or removed argument) rejects the rewrite outright. This
// is what guarantees the text that actually runs can only ever differ from
// the text the model asked for by an `rtk` prefix, matching DSH's own
// `pre-tool-input-rewrite` design principle (that the executed and audited
// text may only differ by a declared filter), even though DSH does not
// implement that proposal yet.

import { execFile } from "node:child_process"
import { existsSync, mkdirSync, appendFileSync } from "node:fs"
import { homedir } from "node:os"
import { dirname, join } from "node:path"

export const name = "rtk-rewrite"

// Cordis only calls `apply` once `ctx.shell` is mounted — without this the
// first (and every) rewrite attempt would silently miss the real service.
export const inject = ["shell"]

/**
 * Subcommands RTK's own `--help` lists that are safe acceptance targets for
 * an inserted `rtk <name>` prefix — i.e. subcommands that wrap a specific,
 * unambiguous external CLI tool name a model might type directly (`git`,
 * `cargo`, `pytest`, …), or RTK's own non-ambiguous output filters (`tree`,
 * `diff`, `find`, `log`, `json`).
 *
 * Deliberately excludes:
 * - `ls`, `env` — PowerShell aliases/builtins with non-text (object-stream)
 *   semantics; rewriting them would change what the command returns, not
 *   just how it is logged.
 * - RTK's own heuristic/meta wrappers that are not 1:1 mappings of an
 *   external tool name a model would type bare (`smart`, `err`, `test`,
 *   `deps`, `summary`, `config`, `gain`, `init`, `rewrite`, `hook`, …). These
 *   never appear as rewrite targets in practice (RTK's registry maps
 *   specific external invocations to specific wrapper subcommands, not to
 *   these), so excluding them costs nothing and keeps the whitelist tight.
 *
 * `read` IS included despite being a bash builtin name, after checking
 * RTK's registry directly: a bare `read`/`read -p` (the builtin's actual
 * use, reading a line into a shell variable) never matches any RTK rewrite
 * rule (`rtk rewrite -- "read line"` exits 1, unchanged) — the only way
 * `rtk read …` appears is as a rewrite of `cat`/`head`/`tail`-style
 * invocations, which is exactly the token-insertion case this plugin should
 * accept. Excluding `read` would only disable those optimizations for no
 * real safety benefit.
 *
 * Source: `target/release/rtk --help` "Commands:" section, filtered per the
 * above. Re-verify against the shipped `rtk` binary's `--help` when bumping
 * this list — RTK's subcommand set changes across releases.
 */
export const DEFAULT_COMMANDS = [
  "git", "gh", "glab", "aws", "psql", "pnpm", "dotnet", "docker", "kubectl",
  "oc", "grep", "rg", "ast-grep", "wget", "wc", "jest", "vitest", "ctest",
  "prisma", "tsc", "ng", "next", "lint", "prettier", "format", "playwright",
  "cargo", "npm", "npx", "bun", "bunx", "curl", "ruff", "sqlfluff", "pytest",
  "mypy", "php", "phpunit", "phpstan", "pest", "paratest", "ecs", "pint",
  "phpt", "rake", "rubocop", "rspec", "pip", "uv", "deno", "go", "sbt", "gt",
  "golangci-lint", "gradlew", "mvn", "mvnd", "find", "diff", "log", "json",
  "tree", "read",
]

/** Inserted-token names `isSafeRewrite` treats as "this is the RTK prefix". */
const RTK_TOKENS = new Set(["rtk", "rtk.exe"])

/** Shell operators tokenized as their own tokens; everything else splits on whitespace. */
const OPERATOR_CHARS = new Set(["&", "|", ";"])

/**
 * Splits a shell command line into tokens, treating `&&`, `||`, `;`, `|`,
 * and `&` as their own tokens (each operator character its own token when
 * not doubled) and preserving whitespace inside `'…'`/`"…"` quoted spans as
 * part of one token (so `git commit -m "fix bug"` stays three tokens, not
 * four). Quotes are kept in the token text — this function is only used to
 * diff two command strings against each other, not to re-parse arguments,
 * so exact text (quotes included) is what must match.
 *
 * @param {string} cmd
 * @returns {string[]}
 */
export function tokens(cmd) {
  const out = []
  let buf = ""
  let quote = null // active quote char, or null
  const flush = () => {
    if (buf !== "") {
      out.push(buf)
      buf = ""
    }
  }

  for (let i = 0; i < cmd.length; i++) {
    const c = cmd[i]
    if (quote) {
      buf += c
      if (c === quote) quote = null
      continue
    }
    if (c === '"' || c === "'") {
      quote = c
      buf += c
      continue
    }
    if (OPERATOR_CHARS.has(c)) {
      flush()
      const next = cmd[i + 1]
      if ((c === "&" && next === "&") || (c === "|" && next === "|")) {
        out.push(c + c)
        i++
      } else {
        out.push(c)
      }
      continue
    }
    if (/\s/.test(c)) {
      flush()
      continue
    }
    buf += c
  }
  flush()
  return out
}

/**
 * True when `rewritten` is exactly `original` with one or more extra literal
 * `rtk` (or `rtk.exe`) tokens inserted — no token removed, added, or changed
 * otherwise. Greedy left-to-right: at each position, a literal match against
 * the next original token is always preferred over treating the rewritten
 * token as an insertion, so a literal `rtk` argument already present in the
 * original (`git log --grep rtk`) is matched as itself, not miscounted as an
 * inserted prefix.
 *
 * @param {string} original
 * @param {string} rewritten
 * @returns {boolean}
 */
export function isSafeRewrite(original, rewritten) {
  const orig = tokens(original)
  const rew = tokens(rewritten)
  let i = 0
  let j = 0
  while (j < rew.length) {
    if (i < orig.length && rew[j] === orig[i]) {
      i++
      j++
      continue
    }
    if (RTK_TOKENS.has(rew[j])) {
      j++
      continue
    }
    return false
  }
  return i === orig.length
}

/**
 * Every subcommand name immediately following an inserted `rtk`/`rtk.exe`
 * token in `rewritten`, given it already passed {@link isSafeRewrite}
 * against `original`. Used to gate the rewrite against the whitelist: if any
 * one of these names is not recognized, the whole rewrite is rejected (never
 * partially applied).
 *
 * @param {string} original
 * @param {string} rewritten
 * @returns {string[]}
 */
export function insertedSubcommands(original, rewritten) {
  const orig = tokens(original)
  const rew = tokens(rewritten)
  const names = []
  let i = 0
  let j = 0
  while (j < rew.length) {
    if (i < orig.length && rew[j] === orig[i]) {
      i++
      j++
      continue
    }
    if (RTK_TOKENS.has(rew[j])) {
      j++
      if (j < rew.length) names.push(rew[j])
      continue
    }
    break
  }
  return names
}

/**
 * Invokes `rtk rewrite -- <command>` and returns the rewritten command, or
 * `undefined` when there is nothing to accept.
 *
 * Only exit 0 (allow-covered rewrite) and 3 (ask/partial-allow rewrite) carry
 * a usable rewrite — see the exit-code table in `src/hooks/rewrite_cmd.rs`.
 * Exit 1 (no rewrite) and 2 (deny) intentionally return `undefined` too: a
 * deny is left to DSH's own permission system, never simulated here (this
 * plugin has no way to actually stop `ctx.shell.execute` from completing the
 * call, so pretending to block it would just desync what the plugin claims
 * from what actually ran). Any spawn failure, non-zero/unexpected exit, or
 * timeout also resolves to `undefined` — never throws.
 *
 * @param {string} rtkBin
 * @param {string} command
 * @param {number} timeoutMs
 * @returns {Promise<string | undefined>}
 */
export function askRtk(rtkBin, command, timeoutMs) {
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["rewrite", "--", command],
      { encoding: "utf8", timeout: timeoutMs, windowsHide: true },
      (error, stdout) => {
        // execFile reports a non-zero exit through `error.code` (and sets
        // `error.signal` instead when the process was killed, e.g. by our
        // own `timeout` option, in which case `code` is absent/undefined).
        // Only 0 and 3 carry a usable rewrite per the exit-code contract
        // (src/hooks/rewrite_cmd.rs) — everything else, including a killed
        // process, falls through to `undefined`.
        if (error && error.code !== 3) {
          resolve(undefined)
          return
        }
        const rewritten = String(stdout ?? "")
        resolve(rewritten.length > 0 ? rewritten : undefined)
      }
    )
  })
}

/** Default path for the global off switch: present (any content) disables the plugin. */
export function defaultBypassPath() {
  return join(homedir(), ".dsh", "rtk", "off")
}

/** Default path for the optional rewrite log (`config.log: true`). */
export function defaultLogPath() {
  return join(homedir(), ".dsh", "rtk", "rewrites.jsonl")
}

/**
 * Best-effort append of one JSON line recording a rewrite. Never throws —
 * logging is strictly observability, not part of the rewrite decision, so a
 * failure here (missing directory, permissions, full disk) must not affect
 * whether the command runs.
 *
 * @param {string} logPath
 * @param {{ original: string, rewritten: string }} entry
 */
function logRewrite(logPath, entry) {
  try {
    mkdirSync(dirname(logPath), { recursive: true })
    appendFileSync(
      logPath,
      JSON.stringify({ ts: new Date().toISOString(), ...entry }) + "\n"
    )
  } catch {
    // Fail open: a broken log must never surface as a broken command.
  }
}

/** Symbol cordis stashes the real service instance behind on every tracked proxy access. */
const CORDIS_ORIGINAL = Symbol.for("cordis.original")

/**
 * Normalizes plugin config with defaults. Every field is optional.
 *
 * @param {object} [config]
 * @returns {{ enabled: boolean, bin: string, commands: Set<string>, timeoutMs: number, log: boolean, bypassPath: string, logPath: string }}
 */
function resolveConfig(config) {
  const cfg = config ?? {}
  return {
    enabled: cfg.enabled ?? true,
    bin: cfg.bin ?? "rtk",
    commands: new Set(cfg.commands ?? DEFAULT_COMMANDS),
    timeoutMs: cfg.timeoutMs ?? 3000,
    log: cfg.log ?? false,
    bypassPath: cfg.bypassPath ?? defaultBypassPath(),
    logPath: cfg.logPath ?? defaultLogPath(),
  }
}

/**
 * Mounts the rewrite patch onto `ctx.shell.execute`.
 *
 * `ctx.shell` is a cordis-tracked service proxy: every *access* returns a
 * fresh traceable wrapper, not the same object twice, so patching whatever
 * `ctx.shell` happens to evaluate to on this particular access would only
 * shadow that one reference — the real executor's own `execute` would be
 * untouched and every other access would still call the unpatched original.
 * `Symbol.for("cordis.original")` unwraps the proxy to the actual service
 * instance, which is where the patch has to live.
 *
 * @param {import("@deepseek-ai/cordis").Context} ctx
 * @param {object} [config]
 */
export function apply(ctx, config) {
  const { enabled, bin, commands, timeoutMs, log, bypassPath, logPath } =
    resolveConfig(config)

  if (!enabled) return

  const target = ctx.shell?.[CORDIS_ORIGINAL] ?? ctx.shell
  if (!target || typeof target.execute !== "function") {
    ctx.logger?.warn?.(
      "rtk-rewrite: ctx.shell has no execute() to patch; plugin inactive"
    )
    return
  }

  const original = target.execute

  async function wrapped(spec, ...rest) {
    try {
      if (existsSync(bypassPath)) return original.call(target, spec, ...rest)
      if (!spec || typeof spec.command !== "string" || spec.command.trim() === "") {
        return original.call(target, spec, ...rest)
      }

      const command = spec.command
      const rewritten = await askRtk(bin, command, timeoutMs)
      if (!rewritten || rewritten === command) {
        return original.call(target, spec, ...rest)
      }
      if (!isSafeRewrite(command, rewritten)) {
        return original.call(target, spec, ...rest)
      }
      const subcommands = insertedSubcommands(command, rewritten)
      if (subcommands.length === 0 || !subcommands.every((n) => commands.has(n))) {
        return original.call(target, spec, ...rest)
      }

      if (log) logRewrite(logPath, { original: command, rewritten })
      return original.call(target, { ...spec, command: rewritten }, ...rest)
    } catch {
      // Fail open on anything unexpected — never let a plugin bug block the
      // user's or the model's command.
      return original.call(target, spec, ...rest)
    }
  }

  target.execute = wrapped

  // Reload/unload must restore the pristine method, or a hot-reloaded
  // plugin stacks a second wrapper around the first on every reload.
  ctx.effect(() => () => {
    target.execute = original
  })
}

export default { name, inject, apply, DEFAULT_COMMANDS }

# Awareness level — config design + file contents

Follow-up to PR #3372 (`fix(awareness): slim awareness text`, branch `clean/awareness-file`, base `develop`).

#3372 replaces the 29-line `hooks/claude/rtk-awareness.md` with an 8-line agent-neutral
`hooks/rtk-awareness.md` and points `RTK_SLIM` at it. That slim file becomes `level = "default"`.
This doc prepares the `[awareness]` config table, the three awareness files, the wiring in
`rtk init`, and the exact content of `high` and `full`.

## Principles

- RTK is transparent. With a hook the LLM never types `rtk`, so `default` and `high` never ask it
  to. Only `full` carries the "prefix every command with `rtk`" rule, for agents without a hook
  or users who want the LLM to drive rtk itself.
- Awareness text is loaded every session. Every line costs tokens on every turn.
- Never tell the LLM to avoid, skip, or doubt a command. A retry caused by awareness text is as
  bad as one caused by a filter.
- Recovery is not an awareness topic. Filters already print their own recovery path through the
  shared helpers (`core::tee::tee_and_hint`, `force_tee_hint`, `force_tee_tail_hint`), and the
  `default` paragraph already says "truncated results state their recovery path in their own
  output". Repeating marker formats in awareness would be dead weight.
- The `default` paragraph (output contract) is present verbatim at every level. `high` adds what
  RTK is and its meta commands. `full` adds the activation rule on top.

## Config

File: `~/.config/rtk/config.toml` (`src/core/config.rs`).

```toml
[awareness]
level = "default"           # "default" | "high" | "full"
```

| Level | File | What it adds | Audience |
|---|---|---|---|
| `default` | `hooks/rtk-awareness.md` (from #3372) | Output contract only. | Hook agents. |
| `high` | `hooks/rtk-awareness-high.md` | What RTK is, meta commands (`gain`, `proxy`, `RTK_DISABLED`, `discover`). | Hook agents whose operator wants the LLM to know rtk exists. |
| `full` | `hooks/rtk-awareness-full.md` | `high` + "prefix every command with `rtk`". | Agents without a hook, or operators who want the LLM to drive rtk. |

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AwarenessLevel {
    #[default]
    Default,
    High,
    Full,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AwarenessConfig {
    #[serde(default)]
    pub level: AwarenessLevel,
}

pub struct Config {
    // ...existing tables...
    #[serde(default)]
    pub awareness: AwarenessConfig,
}
```

Unknown value (`level = "max"`) is a TOML parse error, same strictness as `tee.mode`.
Known gap, out of scope here: `init.rs` calls `Config::load().unwrap_or_default()`
(lines ~461/473), so a malformed config silently falls back to defaults during init.
Worth a warning on stderr in the same PR, since a typo in `level` would otherwise
silently install `default` and the user would not know why.

No CLI flag in phase 1. Config is the source of truth; switching level = edit config, re-run
`rtk init -g`. `write_if_changed` already rewrites `RTK.md` when content differs.
Optional phase 2: `rtk init --awareness high` as a one-run override (no persistence).

## Wiring in `src/hooks/init.rs`

```rust
const RTK_AWARENESS_DEFAULT: &str = include_str!("../../hooks/rtk-awareness.md");
const RTK_AWARENESS_HIGH: &str = include_str!("../../hooks/rtk-awareness-high.md");
const RTK_AWARENESS_FULL: &str = include_str!("../../hooks/rtk-awareness-full.md");

fn awareness_content(level: AwarenessLevel) -> &'static str {
    match level {
        AwarenessLevel::Default => RTK_AWARENESS_DEFAULT,
        AwarenessLevel::High => RTK_AWARENESS_HIGH,
        AwarenessLevel::Full => RTK_AWARENESS_FULL,
    }
}
```

Load the level once in `run()` / the mode entry points and thread it through `InitContext`
(it already exists to avoid parameter sprawl; add `awareness: AwarenessLevel`).

### Hook agents vs rules-only agents

- **Hook agents** (Claude Code, Gemini, Copilot VS Code Chat, OpenCode, Pi, Hermes, Droid, Cursor):
  commands are rewritten by `rtk hook`. They get `awareness_content(level)` as configured.
- **Rules-only agents** (Codex, Windsurf, Cline, Kilocode, Antigravity, Copilot CLI, Kimi):
  no hook, the LLM must type `rtk` itself. Writing `default` or `high` there would silently
  disable rtk. They always get `RTK_AWARENESS_FULL`, regardless of `level`. Document it; do not
  error, since one config serves both kinds of agents on the same machine.

### Where each level applies

| Init mode | File written | Today | Phase 1 | Notes |
|---|---|---|---|---|
| `rtk init -g` (Claude Code) | `~/.claude/RTK.md` | `RTK_SLIM` | `awareness_content(level)` | line 1163; fix the hardcoded `(10 lines)` in the success message |
| `rtk init --gemini` | `~/.gemini/GEMINI.md` | `RTK_SLIM` | `awareness_content(level)` | line 4238 |
| `rtk init --codex` | `RTK.md` + `AGENTS.md` ref | `RTK_SLIM_CODEX` | `RTK_AWARENESS_FULL` | rules-only; drops `hooks/codex/rtk-awareness.md` |
| `rtk init` (local) / `--claude-md` / kimi | `CLAUDE.md` / `AGENTS.md` block | `RTK_INSTRUCTIONS` (legacy full table) | unchanged | legacy mode, keep as is |
| `--copilot` | `copilot-instructions.md` | `COPILOT_INSTRUCTIONS` | unchanged phase 1 | Copilot CLI deny-with-suggestion; phase 2 → `full` inside the marker block |
| windsurf / cline / kilocode / antigravity | `rules.md` | per-agent rules files | unchanged phase 1 | phase 2 → `RTK_AWARENESS_FULL`, delete four ~750-byte duplicates |

Phase 2 (separate PR): point every rules-only mode at `RTK_AWARENESS_FULL`, delete
`hooks/{windsurf,cline,kilocode,antigravity}/rules.md` and `hooks/codex/rtk-awareness.md`.
Per-agent READMEs keep their install notes; only the agent-facing text is unified.
Not in phase 1 because each mode has its own write path and tests.

### Rename

`RTK_SLIM` → `RTK_AWARENESS_DEFAULT`, `RTK_SLIM_CODEX` → `RTK_AWARENESS_FULL`.
Tests at lines ~4867/4871/5615 reference the old names; update.

## `hooks/rtk-awareness-high.md` — draft content

Budget: ≤ 21 lines. First paragraph is `hooks/rtk-awareness.md` byte-for-byte. The rest is the
old `hooks/claude/rtk-awareness.md` (meta commands + hook explanation) trimmed to what an LLM
can act on. No command reference table, no name-collision warning.

```markdown
# Command output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.

## About RTK

The condensing is done by RTK (Rust Token Killer), a CLI proxy. A hook
rewrites each shell command to `rtk <cmd>` before it runs; behavior and
exit code are unchanged, only the output is filtered. Commands RTK has no
filter for run as-is.

- `rtk gain` / `rtk gain --history` — token savings, overall and per command.
- `rtk proxy <cmd>` — run a command unfiltered, still tracked.
- `RTK_DISABLED=1 <cmd>` — skip the hook for one command.
- `rtk discover` — find past commands RTK could have condensed.
```

## `hooks/rtk-awareness-full.md` — draft content

Budget: ≤ 25 lines. Activation rule first: for a rules-only agent it is the one thing that must
be read. Output-contract paragraph verbatim. "About RTK" reworded since there is no hook; the
meta list is the same as `high`.

```markdown
# RTK

Prefix every shell command with `rtk`: `rtk git status`, `rtk cargo test`,
`rtk npm run build`, `rtk ls src/`. Keep the prefix inside chains:
`rtk git add . && rtk git commit -m "msg"`. Commands RTK has no filter for
run as-is, so the prefix is always safe.

# Command output

Command output here is condensed to save tokens, keeping every signal and
dropping costly noise. Treat it as the complete result: run commands
normally, and batch related commands into one call to avoid extra turns.
Truncated results state their recovery path in their own output. Re-run a
command as `rtk proxy <cmd>` only when its result is unusable: empty when
output was clearly expected, contradicting its exit code, or garbled.

## About RTK

RTK (Rust Token Killer) is a CLI proxy that filters command output to save
tokens; behavior and exit code are unchanged.

- `rtk gain` / `rtk gain --history` — token savings, overall and per command.
- `rtk proxy <cmd>` — run a command unfiltered, still tracked.
- `RTK_DISABLED=1 <cmd>` — skip RTK for one command.
- `rtk discover` — find past commands RTK could have condensed.
```

### Left out of every level, on purpose

- The per-command savings table from `RTK_INSTRUCTIONS`. The hook decides coverage; the LLM
  gains nothing from knowing `gh pr view` saves 87%. Still available via `--claude-md`.
- Install verification (`rtk --version`, `which rtk`, name-collision warning). Operator
  debugging; belongs in `rtk init` output or `rtk diagnose`.
- Recovery marker formats. Filters print them; the `default` paragraph tells the LLM to follow them.
- rtk-native commands (`summary`, `err`, `test`, `log`, `json`). Not awareness; if ever wanted,
  that is a separate "rtk toolbox" section the operator opts into, not part of a level.

### Verified while drafting

- `rtk <unknown>` runs the command raw: `main.rs:1283` `run_fallback` executes any non-meta unknown
  subcommand (after TOML filter lookup). "Prefix is always safe" in `full` holds.
- `RTK_DISABLED=1` is honored by the hook (`registry::cmd_has_rtk_disabled_prefix`) and documented
  in `docs/guide/getting-started/configuration.md`.
- `rtk proxy` records the command in tracking with 0% reduction (CLAUDE.md, Proxy Mode).
- `rtk discover` reads Claude Code history (`src/discover/`). For non-Claude agents the line is
  harmless but useless; acceptable cost for one shared file, or drop it in phase 2 if per-agent
  variants come back.

## Tests

`src/core/config.rs`:
- `[awareness] level = "high"` → `High`; `"full"` → `Full`.
- Missing table → `Default`. Empty `[awareness]` → `Default`.
- `level = "max"` → `Err`.
- Round-trip: `Config::default()` serializes `[awareness]\nlevel = "default"` (so `rtk config` shows it).

`src/hooks/init.rs`:
- `awareness_content` maps each variant to its constant.
- `HIGH.starts_with(DEFAULT.trim_end())` — `high` is `default` plus a tail.
- `FULL.contains(DEFAULT.trim_end())` — the output contract is verbatim in `full`.
- `DEFAULT` and `HIGH` do not contain `Prefix every shell command`; `FULL` does.
- All three contain `rtk proxy <cmd>`; `HIGH` and `FULL` contain `RTK_DISABLED=1` and `rtk gain`.
- Line budgets: `HIGH.lines().count() <= 21`, `FULL.lines().count() <= 25` — bloat guards.
- Default-mode init with `level = "high"` writes `RTK.md` == `RTK_AWARENESS_HIGH`; with
  `"full"` == `RTK_AWARENESS_FULL` (mirror of the existing test at ~4867).
- Codex mode writes `RTK_AWARENESS_FULL` for every level, including `default` and `high`.
- Switching level between two inits rewrites the file (`write_if_changed` returns `true`).

Existing test at ~4786 iterates commands and asserts `RTK_INSTRUCTIONS.contains(cmd)`;
it must not be extended to the awareness files (they intentionally omit the command list).

## Docs to touch

- `docs/guide/getting-started/configuration.md`: add the `[awareness]` table to "Full config
  structure" with the level table above, plus the rules-only-agents-always-get-`full` rule.
- `docs/guide/getting-started/supported-agents.md`: mark each agent as hook or rules-only, and
  which file it receives.
- `hooks/README.md` and `hooks/claude/README.md`: #3372 already says `../rtk-awareness.md` is
  shared; add the `-high` and `-full` siblings and the level mapping.
- `CHANGELOG` entry under the version that ships it.

## Order of work

1. Wait for #3372 to land on `develop` (this builds on its file move).
2. `AwarenessConfig` + `AwarenessLevel` + tests.
3. `hooks/rtk-awareness-high.md`, `hooks/rtk-awareness-full.md`, `awareness_content`,
   `InitContext.awareness`, wiring at lines 1163 / 4238 / 2414, success-message line count.
4. Tests listed above, docs.
5. `cargo fmt --all && cargo clippy --all-targets && cargo test --all`.
6. Phase 2 PR: rules-only modes (windsurf, cline, kilocode, antigravity, copilot) → `RTK_AWARENESS_FULL`,
   delete the per-agent duplicates.

## Side finding (filters, not awareness)

Filters calling a tee hint helper today: aws (8 sites), dotnet (7), container (6), cargo (5),
lint (4), glab/curl (3), search/uv/pnpm/go/gh (2), rubocop/ruff/playwright/gt/git (1).
Filters that truncate with a bare `... N more lines/files` and no path break the `default`
paragraph's promise ("truncated results state their recovery path"). Audit candidates for
`force_tee_tail_hint` adoption; separate pass.

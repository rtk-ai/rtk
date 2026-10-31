---
title: Plugins
description: Write your own pipeline method in Rust and build it into rtk — the Method trait, the rtk-plugin SDK, and the rtk plugins workflow
sidebar:
  order: 10
---

# Plugins — your own methods and task kinds

A **method** is the unit of work in a pipeline (`ansi`, `dedup`, `cap`, `toon`,
…). A **plugin** is a package *you* write that provides one or more new methods.
The built-ins cover common output; a plugin is for a transform only you need —
masking a proprietary secret format, collapsing a house log style, reshaping a
bespoke tool's output.

Decisions are different from transforms. If you want an AI/LLM judgment ("is this
worth compressing?", "which model should serve this?"), that is a
[`[classifier.*]`](./conditions.md#judges--conditions-on-meaning) in your `pipeline.toml`, not a
plugin. Plugins are for *transforming* a block of text.

## The workflow

```bash
rtk plugins new shout      # scaffold a method crate in ~/.config/rtk/plugins/
# edit shout/src/lib.rs — fill in apply()
rtk plugins build          # compile a custom rtk with your method linked in
rtk plugins list           # show your plugin crates and what this binary carries
```

Then name the method in a rule, exactly like a built-in:

```toml
[[hook.after_tool.shell]]
name = "loud-errors"
use  = ["shout"]
```

`rtk plugins build` generates a small host crate that depends on `rtk` and your
plugin crates, registers each method, and calls `rtk::entry()`; it compiles with
`cargo` and installs the result (a binary still named `rtk`) to `~/.cargo/bin`.
Nothing about the engine, the hooks, or installation changes — a plugin is
purely additive.

## What you write

A method implements one trait from the `rtk-plugin` SDK. The scaffold gives you
an identity transform to fill in:

```rust
use rtk_plugin::{rtk_plugin, Block, Method, MethodCtx, MethodInfo, Outcome};

struct Shout;

impl Method for Shout {
    fn name(&self) -> &'static str {
        "shout"
    }

    fn info(&self) -> MethodInfo {
        MethodInfo::LOSSLESS_TRANSFORM
    }

    fn apply(&self, block: &Block, _ctx: &MethodCtx) -> Outcome {
        Outcome::Continue(block.text.to_uppercase())
    }
}

rtk_plugin!(Shout);
```

- `apply` receives the block's `text` (plus its subject, format, and provenance)
  and returns an `Outcome`: `Continue(String)` to pass the transformed text down
  the chain, `ShortCircuit(String)` to stop the chain, or `Skip` to leave the
  block untouched.
- `info()` is the contract the engine type-checks at load: `lossy`, `recovery`
  (how a reader gets the original back, e.g. `Recall` or `None`), `cost`, and what the method consumes/produces. Use
  `MethodInfo::LOSSLESS_TRANSFORM` as the base and override only what differs —
  a lossy method must declare its recovery so the original can be restored:
  by default from the recall store (`rtk recall <hash>`), or from files under
  the tee directory if you set `[retriever] mode = "tee"`.
- `rtk_plugin!(…)` lists what the crate exports so the build can register it.
  A crate may export several methods: `rtk_plugin!(Shout, Whisper);`. To export
  task kinds too (or instead), use the long form:
  `rtk_plugin!(methods: [Shout], kinds: [Checklist]);` — `rtk plugins build`
  registers both.

`rtk pipeline validate` checks a plugin method like any other — a bad contract
(a terminal method not last, a `Slow` method at PreToolUse, a format it cannot
consume) fails at build, with the method name.

## Your own task kinds

Methods transform content; **task kinds** define what a delegated worker does.
The built-ins are `summarize` (answer a question about files), `generate`
(write one file from a spec), and `digest` (condense command output). A plugin
can register a new kind the same way it registers a method:

```rust
use rtk_plugin::{rtk_plugin, CorpusFile, TaskIntent, TaskKind, TaskOutcome, WorkerRequest};

struct Checklist;

impl TaskKind for Checklist {
    fn name(&self) -> &'static str { "checklist" }
    fn usage(&self) -> &'static str { "rtk <task> --question \"...\" <files>" }
    fn default_instructions(&self) -> &'static str { "Answer as a checklist." }
    fn assemble(&self, intent: &TaskIntent, corpus: &[CorpusFile],
                instructions: Option<&str>, prompt: Option<&str>)
        -> Result<WorkerRequest, String> { /* build the worker prompt */ }
    fn handle(&self, output: &str) -> TaskOutcome {
        TaskOutcome::Print(output.trim().to_string())
    }
}

rtk_plugin!(methods: [], kinds: [Checklist]);
```

The `rtk_plugin!` line is what `rtk plugins build` wires into the binary. Once registered,
`[task.review] kind = "checklist"` validates, `rtk review`
exists as a CLI command, and rules can `action = "route"` to it. Names must be
unique — built-in kinds cannot be shadowed. A pipeline that declares a task
with a kind your build does not have fails validation (E014).

## Upgrading rtk

A plugin is compiled into your custom `rtk` binary, so installing a new rtk
release replaces that binary and your plugins with it. After every upgrade:

1. In each plugin's `Cargo.toml`, set the `rtk-plugin` dependency's `tag` to
   the new version (`rtk --version` → `vX.Y.Z`). `rtk plugins new` writes the
   tag once, at scaffold time.
2. Run `rtk plugins build` again.

Until you rebuild, a pipeline rule that uses a plugin method fails open: it gets
W014 and is skipped while the rest of the file stays active. A control rule (one
with `action =`) that depends on it is an error (E007) and its file is not
loaded. `rtk pipeline validate` shows which rules are affected.

## Requirements and notes

- A Rust toolchain (`cargo`) on PATH, and network access on the first build —
  the scaffolded `Cargo.toml` depends on `rtk-plugin` by git, pinned to the tag
  matching your installed `rtk`, so your method always links the exact engine
  your `rtk` runs.
- A plugin adds methods and task kinds, not conditions. Every field of a
  tool's input is already a condition via the `input.*` prefix
  (`when."input.subagent_type" = "Explore"`), and AI-driven conditions are
  classifiers.
- A project-local `.rtk/plugins/` is trust-gated like every other project file;
  review with `rtk trust` before it is built.

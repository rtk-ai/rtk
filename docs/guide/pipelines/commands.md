---
title: Pipeline Commands
description: Inspect, validate, explain, benchmark and share compression pipelines with rtk pipeline
sidebar:
  order: 8
---

# Pipeline Commands

`rtk pipeline` is how you inspect and debug the compression engine: what pipeline is active, what it does to a given input, and why a particular rule fired. See [Pipelines](./overview.md) for the concepts and [Methods](./methods.md) for the transforms themselves.

:::note[Full edition only]
`rtk pipeline`, `rtk methods` and `rtk tasks` are part of the compression engine and are absent from the Apache-2.0 edition. See [Editions and Licensing](../resources/editions.md).
:::

## Quick reference

```bash
rtk pipeline show              # print the resolved pipeline
rtk pipeline validate          # validate every pipeline file, non-zero on error
rtk pipeline test              # parse, resolve and validate every loaded file
rtk pipeline status            # last recorded pipeline load problem, if any
rtk pipeline explain           # what fires for a given agent/tool, and why
rtk pipeline bench <file>      # before/after tokens over a local file
rtk pipeline share             # paste-ready markdown card of a pipeline
rtk pipeline share publish     # publish it: GitHub repo (recommended) or gist
rtk pipeline add <source>      # adopt a shared pipeline: validate, install, report what it needs
rtk pipeline search [query]    # find shared pipelines on GitHub (topic rtk-pipeline)
rtk pipeline promote <name>    # move a shadow install live (--discard to drop it)
rtk pipeline eval <name>       # shadow-evaluate an installed pipeline on your live sessions
rtk pipeline log tail          # the opt-in decision log
```

## Inspecting what is active

```bash
rtk pipeline show
```

Prints the pipeline that actually resolved for this directory. To print a built-in verbatim instead, name it:

```bash
rtk pipeline show rtk:default
rtk pipeline show rtk:posthook
```

## Validating your own pipelines

`validate` exits non-zero on errors, which makes it usable as a pre-commit or CI check:

```bash
rtk pipeline validate
```

`test` is the broader check — it parses, resolves and validates every pipeline file RTK loads, not just the active one. When RTK silently fell back because a file failed to load, `status` is where the reason is recorded:

```bash
rtk pipeline test
rtk pipeline status
```

Validation never writes anything into the agent's session. If a file fails to load while the agent is working, RTK drops **that file only** — your other pipeline files, their deny rules included, stay active — records the problem silently, and shows it the next time you run `rtk gain`, `rtk pipeline status` or `validate`. Every code is listed in [Errors and Warnings](./errors.md).

## Explaining a decision

`explain` answers "why did this get compressed the way it did?" for a hypothetical input:

```bash
rtk pipeline explain --agent claude-code --tool Bash --command "git log -20"
rtk pipeline explain --tool Read --file src/main.rs
rtk pipeline explain --tool Read --file big.json --ranged
```

| Flag | Meaning |
|---|---|
| `--agent` | Agent to explain for (`claude-code`, `cursor`, `cline`, …) |
| `--tool` | Tool name (`Bash`, `Read`, `Grep`, `WebFetch`, …). Defaults to `Bash`, or `Read` with `--file` |
| `--file` | File path, for the `before_tool.read` conditions (file_lines, extension, …) |
| `--ranged` | With `--file`: simulate a ranged read rather than a whole-file read |
| `--format` | Content format (`term`, `json`, `web`, `matches`, `lockfile`) |
| `--command` | Command line, for the `command_*` conditions |
| `--last` | Explain the most recent real decision from the log instead of a hypothetical |

`--file` reads the same facts the hook reads, so a dry run cannot disagree with the live decision. Pipe output in to supply the after-tool size conditions (`bytes`, `lines`, `tokens`); without it a rule gated on output size cannot match:

```bash
rtk pipeline explain --tool Grep < output.txt
```

It reports the position, the rule that fired and why, the methods applied, tokens before and after, the recovery hint — and, for a rule that cannot run on your agent, which capability is missing.

`--last` is usually the fastest route when something surprising just happened — it needs the decision log enabled.

## Shadow mode

```toml
[[hook.after_tool.shell]]
name   = "gentle-term"
when   = { is_rtk_command = false }
use    = ["ansi", "dedup"]
shadow = true
```

The rule runs, its decision and would-be savings are logged, nothing is emitted. `rtk gain --shadow` shows what it would have saved; turn `shadow` off when the numbers convince you. Transforms only, and hook rules only: a `shadow` proxy rule is skipped (W018). To try a whole shared pipeline the same way, see [shadow installs](#trying-a-pipeline-silently-shadow-installs).

## Measuring a pipeline

`bench` runs the resolved chain over a local file and reports before/after tokens:

```bash
rtk pipeline bench my-output.txt
```

Use it to check whether a pipeline change helps on your own data.

## The decision log

Off by default. When enabled it records **decisions and commands, never content**:

```bash
rtk pipeline log on
rtk pipeline log tail                    # most recent 20 entries
rtk pipeline log tail --limit 100
rtk pipeline log show --rule my-rule     # filter by rule name
rtk pipeline log show --limit 200
rtk pipeline log off
```

Each entry records timestamp, session, position, tool category, the command as the agent issued it, the rule that fired, action or methods, tokens before and after, shadow flag and recovery hint — never a tool's output, a file's contents or a directive's text. It lives at `~/.local/share/rtk/pipeline.log` (JSON lines).

```text
12:04:11  hook.after_tool.shell   cargo test        term-default    term        3,812 → 640   recall 1f3a9c
12:04:15  hook.before_tool.read   src/Service.java  shunt-large-read route → bulk-read
12:04:31  hook.after_tool.shell   rtk bulk-read …   —               skipped: is_rtk_command
```

Enable it while debugging a pipeline, then turn it off — it is the input `rtk pipeline explain --last` reads.

## Recovery reporting

RTK counts how often the agent asks for an original back. Reads of the recall store are recorded per tool and reported by `rtk gain --recalls` and the recovery-reads table in `rtk gain --by-method`.

It is reporting only: RTK never disables a rule on its own.

`rtk gain` also breaks savings down `--by-rule`, `--by-method`, `--by-layer` and `--shadow`; directives appear net of their standing cost and tasks net of worker cost. See [Token Savings Analytics](../analytics/gain.md).

## Sharing a pipeline

`share` renders a pipeline's measured impact and the pipeline file itself as one artifact. On a terminal it draws a clean, screenshot-able card; piped or redirected — or with `--md` — it emits the paste-ready **markdown** card instead (so `rtk pipeline share > card.md` keeps working), and the two renderings never mix on one stream.

```bash
rtk pipeline share              # the active pipeline (terminal card)
rtk pipeline share rtk:default  # a named one
rtk pipeline share --md         # the markdown card, even on a terminal
```

The card has two panels. **What it does** is derived from the resolved rules, never hand-written. **Measured impact** reports percentages per lever — command filtering and agent hooks as the share of a tool's output bytes removed, delegated reads as the absolute context a worker kept out of your window, and prompt-cache as the share of input served from cache. Figures are estimates (≈ bytes/4): ratios are reliable, absolutes approximate. Percentages travel between setups; token counts and costs do not, which is why the card leads with percentages.

### Publishing

`rtk pipeline share publish` is the interactive publisher. It offers two channels and walks you through validate → secret scan → preview of what leaves your machine → one confirmation:

- **GitHub repository** (recommended) — one public repo per pipeline, in **your** account: `pipeline.toml` plus a README card. The repo description carries the one-line install command, the repo gets the `rtk-pipeline` topic so `rtk pipeline search` finds it, and stars on [github.com/topics/rtk-pipeline](https://github.com/topics/rtk-pipeline) are the leaderboard.
- **GitHub repository (private)** — same repo, visible only to accounts you grant access. For a team or your own machines: anyone with access installs it with the same `rtk pipeline add github:you/repo` line (their `gh` login provides the access). No topic, no search listing — and private is one visibility toggle away from public, so the secret scan and the warnings apply unchanged.
- **GitHub gist** — a single public link with no discovery. Also available non-interactively as `rtk pipeline share --gist`.

Other hosts: publishing targets GitHub, but **installing** does not — `rtk pipeline add <url>` accepts a raw file URL from anywhere (GitLab raw links included), so nothing stops a pipeline living wherever your team keeps code.

```bash
rtk pipeline share publish       # interactive: choose repo or gist, confirm, publish
rtk pipeline share --gist        # non-interactive gist; prints: rtk pipeline add <url>
```

Everything goes through your own `gh` CLI — RTK never sees or stores a token. If `gh` is missing or logged out, the publisher prints the exact manual steps instead. Publishing the same pipeline again updates the same repo, so its link and stars survive. If the pipeline was itself installed from someone else's repo, the publisher offers to **fork** that repo instead of creating a fresh one, keeping the attribution chain visible on GitHub.

A pipeline that declares [dependencies](#dependencies) shows a notice before it is shared: rtk-ai verifies and approves every pipeline before it enters the catalog, and refuses pipelines from untrusted sources and dependencies it cannot verify. Anyone who adds yours from the share installs its dependencies at their own risk, after RTK asks them.

What leaves your machine: the pipeline file and the aggregate percentages from your share card. No commands, no paths, no history — and every number is labeled **self-reported** on the published card.

:::caution[Publishing is public, permanent, and your responsibility]
A repo or gist is public, permanent, and indexable. **You** are responsible for its contents — RTK does not review or endorse shared pipelines. Your pipeline file is published **verbatim**, including any `[upstream.*]` blocks (`base_url`, `api_key_env`), with one line added at the top: `# author: @<login> · shared with rtk pipeline share · <date>`, taken from your `gh` login. Before publishing, a pre-flight scan checks for API-key shapes, tokens, private-key blocks, absolute home paths, and email addresses; a finding blocks publication unless you explicitly type `publish anyway` (or pass `--force`). The scan is a safety net, not a guarantee: read the card yourself before sharing it.
:::

## Adopting a shared pipeline

`add` is the other half of `share`: one command validates a shared pipeline, checks that this computer has what it needs to run (see [What a pipeline needs](#what-a-pipeline-needs)), and installs it. When the file carries an `# author: @login` header, `add` shows it next to the GitHub owner of the repository or gist it came from, and warns when they differ. The header is attribution, not proof: anyone can write any name in it, so the owner is what to trust.

The pipelines reviewed at [rtk-ai.app/pipelines](https://www.rtk-ai.app/pipelines/) can also be picked by name during setup. `rtk init -g` in a terminal lists them, with the one you run now marked `current`, and installs your choice pinned to its reviewed commit, once it has what it needs to run. Choosing another reviewed pipeline later turns off the reviewed one you had, only after the new one is installed, by renaming its file to `<name>.toml.bak`, which RTK does not load, so your edits to it are kept; your own pipeline files are never touched. Choosing `default-bash-compress` goes back to RTK's default with nothing installed. A run that names an agent, or that has no terminal, shows no menu and keeps the current pipeline; pass the name to change it:

```bash
rtk init -g --agent claude --pipeline <name>    # a name from rtk-ai.app/pipelines
```

A pipeline installed this way goes into your user pipelines folder and applies to every project. That is why `rtk init` run for a single project shows no pipeline menu, and why `--pipeline` installs for your user account even without `-g`.

Init fetches the list from `rtk-ai.app/pipelines/index.json` with a 2-second limit and drops revoked entries. When the site cannot be reached, it uses the newer of two copies: the last list this computer fetched, or the list shipped with your rtk version.

### What a pipeline needs

RTK reads what a pipeline needs from its own file, before installing it:

- each **dependency** it declares under `[dependency.<name>]` (see [Dependencies](#dependencies) below). One that something already answers for at its address, such as a copy you run yourself, is met without installing anything. One that a pipeline you already run declares with a different recipe is never met: installing it would replace the copy that pipeline uses;
- each **local service** it calls at a loopback address (`127.0.0.1`, `localhost`, `[::1]`) without a dependency that says how to install it. That one is met only if something already answers there;
- the **RTK daemon**, when the file extends `rtk:proxy` or has proxy rules of its own. The proxy rule that RTK's default ships does not count. The daemon is met when Claude Code's `settings.json` routes to it or it is running, and set up through [the daemon question](../proxy/overview.md#rtk-init--g--the-guided-setup).

API keys are not needs: RTK prints `auth needed: export VAR=...` for any key that is unset (see [Tasks and Upstreams](./tasks.md#upstreams)).

When something is missing, `rtk init -g` (after you pick a pipeline) and `rtk pipeline add` (after its review panel) list each need and its state, then:

- **In a terminal**, they install or ask for each missing piece. If you answer No, or a piece cannot be had (a local service that is not running, `rtk-proxy` not installed, a recipe this computer cannot use), the pipeline is **not installed** and the pipelines you already run stay as they are. A yellow alert names what is missing and the command to run again. A No exits 0; anything that failed exits 1.
- **Without a terminal** (pipes, CI, `--auto-patch`), `rtk init -g --pipeline <name>` installs the dependencies of a [reviewed](https://www.rtk-ai.app/pipelines/) pipeline without asking, after printing the same panel, because you named it. It never sets up the RTK daemon this way, and `--no-patch` installs nothing. Anything still missing, or any pipeline from another source, means the pipeline is not installed, with the alert, and the command exits 1. `rtk pipeline add` without a terminal never installs a dependency.

"Reviewed" means the file on disk is still exactly the one installed from the catalog's reviewed commit: once you edit it, or replace it from another source, it is treated as any other source. If `rtk init -g --pipeline <name>` finds a different file of the same name that rtk did not install, it keeps yours as `<name>.toml.bak` before installing the reviewed one.

A reviewed pipeline you pick again in `rtk init -g` while it is already installed is never turned off. A copy older than the catalog's reviewed commit is updated first; if the update's needs are not met, the installed version stays. Then init offers any dependency it can install, and if a need is still missing (for example a local service that is stopped right now), the alert lists it and the pipeline stays installed. That exits 0 in a terminal and 1 without one, so a CI job notices.

### Dependencies

A pipeline can need a service on your computer, such as a local decision model its judge asks. It declares it under `[dependency.<name>]` in its own file (see [Dependencies](./tasks.md#dependencies)), so a shared pipeline carries it. A hosted service (TypeSafe Jev, hosted Laya, any API) is not a dependency: it is an upstream with a key, and RTK never installs, starts or stops it.

Before installing a dependency, RTK shows what it will install, where it goes, what it runs on which address, and whether rtk-ai reviewed the pipeline:

- A pipeline from the [reviewed catalog](https://www.rtk-ai.app/pipelines/), installed at its reviewed commit, says so.
- A pipeline from any other source (your organization's repository, a gist, a URL, a file) shows a "not reviewed by rtk-ai" panel: you install its dependencies at your own risk. It is not refused.

No flag or environment variable answers for you, except naming a reviewed pipeline with `rtk init -g --pipeline <name>` where no terminal can ask. When `rtk init -g` runs in a terminal without you picking a pipeline, it also asks about any dependency the pipelines you already run still miss; those pipelines keep running without the rules that need it until it is installed.

Every recipe pins an exact version and its hash, and RTK checks the hash before anything runs. For npm that covers the named package; npm picks the package's own dependencies at install time. pip checks every package. RTK records the recipe you accepted and only ever starts that copy; if a pipeline later declares a different recipe, `rtk init -g` asks again.

After that, nothing needs starting or stopping by hand:

- When a tool call needs the dependency and finds it stopped, that call goes ahead without the rules that need it, and RTK starts it in the background for the calls that follow. Claude Code shows a one-line message when this happens. After a failed start, RTK waits 30 minutes before trying again.
- If another program answers on the dependency's address, RTK leaves it alone.
- `rtk pipeline status` shows each dependency: running, starting (with its progress), stopped, not installed, or not installable on this computer.
- Dependencies stay installed when you switch pipelines, so switching back costs nothing; `rtk pipeline status` lists the unused ones. `rtk init -g --uninstall` stops and removes every dependency RTK installed. Data a dependency keeps elsewhere, such as Laya's model weights in `~/.cache/receptron-laya`, is not removed.

The `laya-route.toml` example declares [Laya](https://github.com/receptron/laya), an open-source decision model (Node.js 20+, about 1.7 GB of weights, about 2 GB of RAM while running), this way.

```bash
rtk pipeline add github:owner/repo              # a published repo (see `share publish`)
rtk pipeline add github:owner/repo@<ref>        # a specific commit, tag, or branch
rtk pipeline add card.md                        # a saved share card (the TOML is auto-extracted)
rtk pipeline add - < card.md                    # or pasted on stdin
rtk pipeline add https://example.com/p          # or any URL
rtk pipeline add https://gist.github.com/u/ID   # or a gist link from `share --gist`
rtk pipeline add card.md --name flash           # name it (default: the pipeline's own name, or the file stem)
rtk pipeline add github:owner/repo --shadow     # try it silently instead of installing (see below)
```

A `github:` source (a plain `https://github.com/owner/repo` link works too) fetches the repo's `pipeline.toml` **pinned to a resolved commit** — the commit is recorded locally, so nothing the author pushes later changes what you installed. A `gist.github.com` page link is resolved to its raw content automatically.

It validates in memory first — a pipeline that does not parse or type-check installs **nothing**. On success it writes a named pipeline to `~/.config/rtk/pipelines/<name>.toml` (merged with your others, active immediately), then prints any missing auth as the exact `export` to run, and a `rtk llm-proxy start` line only if the pipeline actually has proxy rules.

On a terminal, after showing the pipeline's rules, `add` offers three choices: **install now**, **try silently first** (a shadow install — see below), or **quit**. Re-adding a name that is already installed never replaces it silently: `add` shows a line diff of the change (and the commit movement, for pinned sources), repeats that **you are responsible for what you pull and use**, and asks. Non-interactively (pipes, scripts, CI) nothing installs without an explicit opt-in: pass `--force` to accept the disclosure, or `--shadow` to try the pipeline without applying anything. The same rule applies to publishing: `share --gist` in a pipe needs `--force`, and the interactive publisher asks for an explicit "I understand" after its warning.

### Trying a pipeline silently (shadow installs)

`--shadow` (or answering `try silently` interactively) installs into a shadow slot: the pipeline is evaluated and **measured** on your live sessions but applied to nothing — no rewrites, no filtering, no denies, no tasks. After a few sessions, compare and decide:

```bash
rtk pipeline eval <name>          # what it would have done, vs your active pipeline, same traffic
rtk pipeline promote <name>       # go live (shows the diff against any installed version, confirms)
rtk pipeline promote <name> --discard   # drop it; nothing was ever applied
```

```text
installed: ~/.config/rtk/pipelines/flash.toml
auth needed: export GEMINI_API_KEY=...   (upstream `flash` — task `bulk-read`)
active now, merged with your other pipelines — see `rtk pipeline show`
```

The shared file names the *environment variable* for each key (`api_key_env`), never the key itself — you supply your own.

:::caution[You are responsible for what you install]
A pipeline directs your agent: it can rewrite the commands it runs, filter and replace what it reads, inject instructions, and send content to any upstream endpoint it declares. `add` prints exactly what the pipeline does — its rules, directives in full, and every upstream with the env var it would send — **before** anything runs. Read that summary, especially for a pipeline fetched from a URL.
:::

:::danger[Pipelines that need plugins run third-party native code]
Some pipelines reference **plugin** methods. A plugin is not sandboxed TOML — it is third-party native Rust that rtk compiles and runs on your machine with your privileges. If a shared pipeline needs a plugin you do not have, `add` names the missing plugin and refuses to install: to use it you must obtain the plugin's source from its author, add it to your build, and rebuild rtk. If the pipeline references a plugin you already have, `add` lists it in the disclosure. Either way, install a plugin only if you trust its author and have read its source — you alone are responsible for any native code you build and run.
:::

## Discovering shared pipelines

```bash
rtk pipeline search              # shared pipelines, most-starred first
rtk pipeline search rust         # filtered by a free-text query
```

Discovery is GitHub's own index of the `rtk-pipeline` topic. Each result shows the repo, its stars, and its description (which carries the author's headline number and the install line). The leaderboard is simply [github.com/topics/rtk-pipeline](https://github.com/topics/rtk-pipeline) sorted by stars. Descriptions and numbers are **self-reported by their authors** — rtk verifies nothing; `rtk pipeline add` shows every rule before anything installs, and `--shadow` lets you measure a pipeline on your own traffic before it touches anything.

## Evaluating a pipeline on your own usage

`eval` answers "would this pipeline do better on *my* workload?" without adopting it and without storing any conversation content. While armed, every tool output your sessions produce is also run through the candidate's transforms **in memory**; only the before/after sizes are recorded, the content is discarded, and your agent's output is untouched.

```bash
rtk pipeline eval squeeze-term --start   # arm the candidate (must be installed — see `add`)
# ...work normally for a few sessions...
rtk pipeline eval squeeze-term           # compare: candidate vs your active pipeline, same traffic
rtk pipeline eval squeeze-term --stop    # disarm
```

```text
Pipeline eval — `squeeze-term` · last 30 days · vs raw, same trajectory
  candidate      5.1K → 3.2K tokens  (37% removed · 214 outputs)   est.
  active now     5.1K → 4.0K tokens  (22% removed · 214 outputs)   est.
  ~ tokens est. ≈ bytes/4; both measured on the same live outputs
  ~ classifier/route effects excluded from the candidate
```

Both lines are measured on the **same** outputs, so the comparison is apples-to-apples. Limits, stated on the report itself: token counts are bytes÷4 estimates; a candidate's classifier- or route-driven rules are excluded (they would need live LLM calls), so eval covers its deterministic transforms.

The continuous counterpart lives in `rtk gain`: a per-mechanism efficiency section (filter+compress, LLM-spend, recalls, directives, proxy bill actual-vs-without) with a confidence tag on every line — `measured` where real provider usage exists, `est.` where it is a size estimate.

## Related commands

```bash
rtk methods list    # every registered method with its contract
rtk tasks list      # every configured task with its kind and upstream
```

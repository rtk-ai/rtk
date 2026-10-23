<p align="center">
  <img src="https://avatars.githubusercontent.com/u/258253854?v=4" alt="RTK - Rust Token Killer" width="500">
</p>

<p align="center">
  <strong>Agent-agnostic token optimization: compress, route, filter, and more.</strong>
  <br><br>
  One composable pipeline across everything your AI agent reads and sends — built on RTK's battle-tested bash-output compression (up to 90% smaller).
</p>

<p align="center">
  <a href="https://github.com/rtk-ai/rtk/actions"><img src="https://github.com/rtk-ai/rtk/workflows/Security%20Check/badge.svg" alt="CI"></a>
  <a href="https://github.com/rtk-ai/rtk/releases"><img src="https://img.shields.io/github/v/release/rtk-ai/rtk" alt="Release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-Apache--2.0%20%2B%20BUSL--1.1-blue.svg" alt="License: Apache-2.0 and BUSL-1.1"></a>
  <a href="https://discord.gg/RySmvNF5kF"><img src="https://img.shields.io/discord/1470188214710046894?label=Discord&logo=discord" alt="Discord"></a>
  <a href="https://formulae.brew.sh/formula/rtk"><img src="https://img.shields.io/homebrew/v/rtk" alt="Homebrew"></a>
</p>

<p align="center">
  <a href="https://www.rtk-ai.app">Website</a> &bull;
  <a href="#installation">Install</a> &bull;
  <a href="https://www.rtk-ai.app/guide/resources/troubleshooting">Troubleshooting</a> &bull;
  <a href="docs/contributing/ARCHITECTURE.md">Architecture</a> &bull;
  <a href="https://discord.gg/RySmvNF5kF">Discord</a>
</p>

<p align="center">
  <a href="README.md">English</a> &bull;
  <a href="README_fr.md">Francais</a> &bull;
  <a href="README_zh.md">中文</a> &bull;
  <a href="README_ja.md">日本語</a> &bull;
  <a href="README_ko.md">한국어</a> &bull;
  <a href="README_es.md">Espanol</a> &bull;
  <a href="README_pt.md">Português</a>
</p>

---

<p align="center">
 <a href="https://www.star-history.com/rtk-ai/rtk">
  <picture><source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=rank&theme=dark" /><source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=rank" /><img alt="Star History Rank" src="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=rank" /></picture> <picture><source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=trending&theme=dark" /><source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=trending" /><img alt="GitHub Trending Repository of the Day" src="https://api.star-history.com/badge?repo=rtk-ai/rtk&type=trending" /></picture>
 </a>
</p>

rtk filters and compresses command outputs before they reach your LLM context. Single Rust binary, 100+ supported commands, <10ms overhead.

Angular builds are supported with `rtk ng build`: compact build tables with every bundle and diagnostic retained.

## What RTK Does

RTK intercepts shell commands and compresses their output before your agent reads it.

| Operation | What RTK does to the output |
|-----------|-----------------------------|
| `ls` / `tree` | Tree format with file counts instead of one line per entry |
| `cat` / `read` | Smart file reading: signatures and structure over full bodies |
| `grep` / `rg` | Truncates long lines, groups matches by file |
| `ast-grep` | Groups structural matches by file, caps overflow |
| `git status` | Compact stat format, grouped by state |
| `git diff` | Reduced context, headers stripped |
| `git log` | Hash, author and subject only |
| `git add/commit/push` | Confirmation line instead of full progress output |
| `cargo test` / `npm test` | Failures only, passing tests collapsed to a count |
| `ruff check` | Grouped by rule and file |
| `sqlfluff lint` | Grouped by rule and file |
| `pytest` | Failures only, traceback trimmed |
| `go test` | NDJSON parsed, failures only |
| `docker ps` | Essential fields only |

## How Savings Work

RTK reduces the **bash output** your agent reads. That is what RTK measures, and it is not the same as cutting your bill by the same amount.

Bash output is **one contributor to input tokens**, alongside your prompt, the system prompt and conversation history. Input tokens are in turn **only part of the bill**, which also counts output tokens. The reduction dilutes at every step.

The token counts RTK reports are estimated as `bytes / 4` — RTK ships no tokenizer, so the **percentages are reliable but the absolute token numbers are approximate**.

> Full explanation: [How RTK Savings Work](docs/guide/resources/savings-explained.md)
## Installation

```bash
brew install rtk                                              # macOS / Linux
winget install rtk-ai.rtk                                     # Windows
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

Then check you have the right package — another project named "rtk" exists on crates.io:

```bash
rtk --version   # any current version
rtk gain        # must show the savings dashboard, not "command not found"
```

> Cargo, pre-built binaries, native Windows, WSL, and uninstall: **[Installation guide](https://www.rtk-ai.app/guide/getting-started/installation)**

## What You Install

RTK ships as three downloads. The two `rtk` editions install a binary with the **same name** — hooks, scripts and agent config work unchanged across both — so they cannot be installed together. Uninstall one before installing the other.

```mermaid
flowchart TD
    agent["Your coding agent"]

    subgraph editions["Pick one · both install a binary named rtk"]
        direction LR
        full["<b>rtk</b><br/>Apache-2.0 + BUSL-1.1<br/>hooks · filters · engine"]
        oss["<b>rtk-oss</b><br/>Apache-2.0 only<br/>hooks · filters"]
    end

    proxy["<b>rtk-proxy</b><br/>BUSL-1.1<br/>compresses API traffic"]

    agent -->|"command output"| full
    agent -->|"command output"| oss
    agent -.->|"API traffic"| proxy

    classDef apache stroke:#2da44e,stroke-width:2px,fill:none
    classDef busl stroke:#0969da,stroke-width:2px,fill:none
    classDef mixed stroke:#8250df,stroke-width:2px,fill:none
    classDef plain stroke:#8c959f,stroke-width:1px,fill:none
    class full mixed
    class oss apache
    class proxy busl
    class agent plain
    style editions fill:none,stroke:#d0d7de,stroke-dasharray:4 4
```

| Download | Licence | Contains |
|---|---|---|
| `rtk-<target>` | Apache-2.0 + BUSL-1.1 | Hook core, every command filter, **and** the compression engine |
| `rtk-oss-<target>` | Apache-2.0 only | Hook core and every command filter. No engine. |
| `rtk-proxy-<target>` | BUSL-1.1 | The RTK Proxy daemon. Never bundled with either `rtk`. |

**Production use is free**, engine and daemon included. What the BUSL grant does not cover is offering RTK or a derivative to third parties as a commercial product or service — for that, contact **contact@rtk-ai.app**.

```bash
curl -fsSL .../install.sh | RTK_EDITION=oss sh    # Apache-2.0 edition
brew install rtk-ai/tap/rtk-oss
```

> What the Apache-2.0 edition omits and where the licence boundary falls: **[Editions and Licensing](https://www.rtk-ai.app/guide/resources/editions)**

## Quick Start

```bash
# 1. Install for your AI tool
rtk init -g                     # Claude Code / Copilot (default)
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
rtk init --agent kilocode       # Kilo Code
rtk init --agent antigravity    # Google Antigravity
rtk init --agent kimi           # Kimi AI
rtk init -g --agent pi          # Pi
rtk init --agent omp            # Oh My Pi (OMP)
rtk init --agent hermes         # Hermes
rtk init -g --agent droid       # Factory Droid
rtk init --agent trae           # Trae (project: .trae/hooks.json)
rtk init -g --agent trae        # Trae global: ~/.trae and existing ~/.trae-cn

# 2. Restart your AI tool, then test
git status  # Automatically rewritten to rtk git status
```

The agent receives compact output without ever calling `rtk` explicitly.

**The hook only fires on Bash tool calls.** Built-in tools like `Read`, `Grep` and `Glob` do not pass through it. Use shell commands (`cat`, `rg`, `find`) or call `rtk read`, `rtk grep`, `rtk find` directly.

> Per-agent setup, `--dry-run`, and uninstall: **[Quick Start](https://www.rtk-ai.app/guide/getting-started/quick-start)** · **[Supported Agents](https://www.rtk-ai.app/guide/getting-started/supported-agents)**

## How It Works

```
  Without rtk:                                    With rtk:

  Claude  --git status-->  shell  -->  git         Claude  --git status-->  RTK  -->  git
    ^                                   |            ^                      |          |
    |         full raw output           |            |  compact output      | filter   |
    +-----------------------------------+            +------- (filtered) ---+----------+
```

Four strategies applied per command type:

1. **Smart Filtering** - Removes noise (comments, whitespace, boilerplate)
2. **Grouping** - Aggregates similar items (files by directory, errors by type)
3. **Truncation** - Keeps relevant context, cuts redundancy
4. **Deduplication** - Collapses repeated log lines with counts

> **Does RTK break Claude's prompt cache?** No. RTK filters output once per command. The result is stored in history and cached normally on subsequent API calls, so the cache keeps working as expected. Smaller outputs also mean cheaper cache writes and reads. See [Troubleshooting](docs/guide/resources/troubleshooting.md#does-rtk-break-claudes-prompt-cache) for details.

## Supported AI Tools

RTK supports 18 AI coding tools. Each integration rewrites shell commands to `rtk` equivalents, reducing the bash output the agent reads where the agent supports command interception.

| Tool | Install | Method |
|------|---------|--------|
| **Claude Code** | `rtk init -g` | PreToolUse hook (native binary) |
| **GitHub Copilot (VS Code)** | `rtk init -g --copilot` | PreToolUse hook — transparent rewrite |
| **GitHub Copilot CLI** | `rtk init -g --copilot` | PreToolUse deny-with-suggestion (CLI limitation) |
| **Cursor** | `rtk init -g --agent cursor` | preToolUse hook (hooks.json) |
| **Gemini CLI** | `rtk init -g --gemini` | BeforeTool hook |
| **Codex** | `rtk init -g --codex` | PreToolUse hook (`updatedInput`) + AGENTS.md |
| **Windsurf** | `rtk init -g --agent windsurf` | .windsurfrules (project-scoped) |
| **Cline / Roo Code** | `rtk init --agent cline` | .clinerules (project-scoped) |
| **OpenCode** | `rtk init -g --opencode` | Plugin TS (execute.before / tool.execute.before), needs 1.18.29 or newer |
| **OpenClaw** | `openclaw plugins install ./openclaw` | Plugin TS (before_tool_call) |
| **Pi** | `rtk init -g --agent pi` (global) | TypeScript extension (tool_call) |
| **Oh My Pi (OMP)** | `rtk init -g --agent omp` (global) / `rtk init --agent omp` (project) | TypeScript extension (tool_call, shared with Pi) |
| **Hermes** | `rtk init --agent hermes` | Python plugin adapter (terminal command mutation via `rtk rewrite`) |
| **Mistral Vibe** | `rtk init -g --agent vibe` | `pre_tool` hook (hooks.toml) |
| **Kilo Code** | `rtk init --agent kilocode` | .kilocode/rules/rtk-rules.md (project-scoped) |
| **Google Antigravity** | `rtk init --agent antigravity` / `rtk init -g --agent antigravity` | .agents/plugins/rtk/ (project) or ~/.gemini/config/plugins/rtk/ (global): plugin.json, hooks.json, rules/AGENTS.md |
| **Kimi AI** | `rtk init --agent kimi` | AGENTS.md (project-scoped) |
| **Factory Droid** | `rtk init -g --agent droid` (or per-project) | PreToolUse hook in `~/.factory/hooks.json` (matcher `Execute`) |
| **Trae** | `rtk init --agent trae` | Native `PreToolUse` hook in `.trae/hooks.json` (`RunCommand`) |
| **Trae (global)** | `rtk init -g --agent trae` | `~/.trae/hooks.json`, plus `~/.trae-cn/hooks.json` when that directory exists |

For per-agent setup details, override controls, and graceful degradation, see the [Supported Agents guide](https://www.rtk-ai.app/guide/getting-started/supported-agents). The Hermes plugin source and tests live in `hooks/hermes/`; installed Hermes runtime files still live under `~/.hermes/plugins/rtk-rewrite/`.
## What RTK Covers

100+ commands across Git, GitHub CLI, Cargo/Rust, JavaScript (npm, pnpm, vitest, jest, tsc, Next.js, Playwright, Prisma), Python (ruff, pytest, mypy, pip), Go (go test, golangci-lint), Ruby, .NET, JVM (Gradle, Maven), PHP, Docker, Kubernetes, AWS, Terraform, and the file/search builtins.

```bash
-v, --verbose          # increase verbosity (-v, -vv, -vvv)
rtk bypass <cmd>       # run one command unfiltered, still tracked
```

> Every command and its filter: **[What RTK Optimizes](https://www.rtk-ai.app/guide/resources/what-rtk-covers)**

## Measure It

```bash
rtk gain                        # Summary stats
rtk gain --graph                # ASCII graph (last 30 days)
rtk gain --history              # Recent command history
rtk gain --daily                # Day-by-day breakdown
rtk gain --all --format json    # JSON export for dashboards

rtk discover                    # Find missed savings opportunities
rtk discover --all --since 7    # All projects, last 7 days

rtk session                     # Show RTK adoption across recent sessions
```

> Attribution, the live dashboard and export formats: **[Token Savings Analytics](https://www.rtk-ai.app/guide/analytics/gain)**

## RTK Proxy (optional)

`rtk` optimizes **command output** before your agent reads it. The `rtk-proxy` daemon extends the same engine to your agent's **provider-API traffic** — a local background process that sits between your agent and the provider and runs the pipeline on the traffic itself: it **compresses** payloads, strips bytes no model ever needed before forwarding, and carries the calls your pipeline routes to cheaper models, so they borrow the credential your agent already sends and show up in `rtk gain`. It can also align prompt-cache breakpoints where the agent leaves them open (opt-in).

```bash
rtk claude              # start the daemon if needed, then launch Claude through it
rtk llm-proxy status    # is it up
rtk gain                # savings, command-filtering and proxy side by side
```

**Two independent ways in**, because not every harness has a session hook:

| | What it does |
|---|---|
| `rtk init -g` | Offers to set it up once: writes the base URL and the `rtk:proxy` pipeline, then asks how the daemon should come back after a reboot — SessionStart hook, background service, or neither. |
| `rtk <harness>` | No configuration at all. Starts the daemon, points the harness at it, then execs it. Needs no hook system, which is why it covers harnesses that have none. |

`rtk <harness>` resolving is itself the signal that traffic really goes through the daemon — the subcommand exists only where redirection is verified. Today that is `rtk claude`. A harness without one is never silently unproxied: it falls through to plain passthrough, and `rtk llm-proxy setup` lists what can be launched.

It is a separate binary under a separate licence (BUSL-1.1), shipped as its own `rtk-proxy-<target>` archive — `rtk` never links it.

> Setup and subcommands: **[RTK Proxy](https://www.rtk-ai.app/guide/proxy/overview)** · what it compresses: **[What the Proxy Compresses](https://www.rtk-ai.app/guide/proxy/compression)** · settings and endpoints: **[Proxy Configuration](https://www.rtk-ai.app/guide/proxy/configuration)**

## The Engine (`pipeline.toml`)

Everything RTK does — which commands get rewritten, what happens to a tool's output, what the proxy compresses — is expressed in one composable file. The engine (`engine/`, crate `rtk-engine`, BUSL-1.1) reads it, so the levers are configurable instead of hard-coded.

**Upgrading changes nothing.** With no pipeline file you are on `rtk:default`: the same PreToolUse rewrite, the same awareness file, your `filters.toml`, your `config.toml`. Everything beyond that is opt-in, one line at a time:

```toml
# ~/.config/rtk/pipeline.toml
[pipeline]
extends = ["rtk:posthook", "rtk:proxy"]
```

> The tour: **[Pipelines](https://www.rtk-ai.app/guide/pipelines/overview)** · the reference: **[Writing a Pipeline](https://www.rtk-ai.app/guide/pipelines/writing-pipelines)**, **[Conditions and Judges](https://www.rtk-ai.app/guide/pipelines/conditions)**, **[Methods](https://www.rtk-ai.app/guide/pipelines/methods)**, **[Tasks and Upstreams](https://www.rtk-ai.app/guide/pipelines/tasks)** · the commands: **[Pipeline Commands](https://www.rtk-ai.app/guide/pipelines/commands)** · write your own method: **[Plugins](https://www.rtk-ai.app/guide/pipelines/plugins)**

## Documentation

**[rtk-ai.app/guide](https://www.rtk-ai.app/guide)** is the full user guide.

| | |
|---|---|
| [Installation](https://www.rtk-ai.app/guide/getting-started/installation) | every install method, Windows, WSL, uninstall |
| [Quick Start](https://www.rtk-ai.app/guide/getting-started/quick-start) | per-agent init, `--dry-run`, first savings |
| [Supported Agents](https://www.rtk-ai.app/guide/getting-started/supported-agents) | setup detail, override controls, degradation |
| [Agent Coverage](https://www.rtk-ai.app/guide/resources/agent-coverage) | what each agent's hook API lets RTK do |
| [What RTK Optimizes](https://www.rtk-ai.app/guide/resources/what-rtk-covers) | every command and its filter |
| [Configuration](https://www.rtk-ai.app/guide/getting-started/configuration) | `config.toml`, env vars, recall recovery |
| [Token Savings Analytics](https://www.rtk-ai.app/guide/analytics/gain) | `rtk gain` in full |
| [Pipelines](https://www.rtk-ai.app/guide/pipelines/overview) | the engine: `pipeline.toml`, rules, judges, tasks, sharing |
| [RTK Proxy](https://www.rtk-ai.app/guide/proxy/overview) | the daemon: setup, what it compresses, settings |
| [Editions and Licensing](https://www.rtk-ai.app/guide/resources/editions) | what each download contains |
| [Telemetry & Privacy](https://www.rtk-ai.app/guide/resources/telemetry) | what is collected, how to opt in, your GDPR rights |
| [Troubleshooting](https://www.rtk-ai.app/guide/resources/troubleshooting) | common issues, prompt-cache questions |

Repository references: [engine/README.md](engine/README.md) · [proxy/README.md](proxy/README.md) · [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md)

RTK collects **no command content and no file contents**. Telemetry is anonymous and **off by default** — it sends nothing until you opt in with explicit consent (GDPR Art. 6 & 7) via `rtk telemetry enable`, and documented in [Telemetry & Privacy](https://www.rtk-ai.app/guide/resources/telemetry).

## Star History

<a href="https://www.star-history.com/?repos=rtk-ai%2Frtk&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=rtk-ai/rtk&type=date&legend=top-left" />
 </picture>
</a>

## StarMapper

<a href="https://starmapper.bruniaux.com/rtk-ai/rtk">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk?theme=light" />
    <img alt="StarMapper" src="https://starmapper.bruniaux.com/api/map-image/rtk-ai/rtk" />
  </picture>
</a>

## Core team

- **Patrick Szymkowiak** — Founder
  [GitHub](https://github.com/pszymkowiak) · [LinkedIn](https://www.linkedin.com/in/patrick-szymkowiak/)
- **Florian Bruniaux** — Core contributor
  [GitHub](https://github.com/FlorianBruniaux) · [LinkedIn](https://www.linkedin.com/in/florian-bruniaux-43408b83/)
- **Adrien Eppling** — Core contributor
  [GitHub](https://github.com/aeppling) · [LinkedIn](https://www.linkedin.com/in/adrien-eppling/)
- **Nicolas Le Cam** — Core contributor
  [Github](https://github.com/kush) · [LinkedIn](https://www.linkedin.com/in/nicolas-le-cam-386387160/)
- **Takayuki Maeda** — Core contributor
  [GitHub](https://github.com/TaKO8Ki) · [LinkedIn](https://www.linkedin.com/in/tako8ki/)

## Contributing

Contributions welcome! Please open an issue or PR on [GitHub](https://github.com/rtk-ai/rtk).

Join the community on [Discord](https://discord.gg/RySmvNF5kF).

## License

The `rtk` CLI is **Apache-2.0** — see [LICENSE](LICENSE).

It links one component under a different licence. `rtk-engine` (`engine/`) is
**BUSL-1.1**, source-available: see [engine/LICENSE](engine/LICENSE). Its
Additional Use Grant permits production use as part of the `rtk` binary, so
running `rtk` is covered; using the engine on its own, or inside another
product, is not until the Change Date.

The `rtk-proxy` daemon (`proxy/`) is **BUSL-1.1** on the same terms and ships as
its own archive with its licence alongside — `rtk` never links it.

## Disclaimer

See [DISCLAIMER.md](DISCLAIMER.md).
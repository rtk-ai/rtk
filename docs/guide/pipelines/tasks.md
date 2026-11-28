---
title: Tasks and Upstreams
description: Delegate reads, commands and writes to a cheaper model — task kinds, instances, upstream endpoints, keys, and what it costs
sidebar:
  order: 6
---

# Tasks and Upstreams

A task is a command the agent can run that hands work to a cheaper model. RTK ships three **kinds** — interfaces with a prompt template and output handling. You create named **instances** of a kind and bind each to a model (an **upstream**). A [plugin](./plugins.md) can register further kinds; a task whose kind this `rtk` does not have fails validation (E014) — tasks sit on the control path.

| Kind | Interface | The agent types | Saves on |
|---|---|---|---|
| `summarize` | files + a question → bullets | `rtk <task> --question "…" <paths…>` | read |
| `digest` | captured output + a question → bullets | `rtk <task> --question "…" -- <command>` | shell, search, web, mcp |
| `generate` | a spec + a reference file → a file written; the agent sees one line | `rtk <task> --spec "…" --reference <f> --target <f>` | write, read |

```toml
[upstream.flash]
provider     = "google"
model        = "gemini-2.5-flash"
api_key_env  = "GEMINI_API_KEY"
timeout_secs = 30

[task.bulk-read]
kind         = "summarize"
upstream     = "flash"
temperature  = 0.2

[task.code-write]
kind     = "generate"
upstream = "flash"
```

The agent reaches a task two ways: a `route` rule hands it a call ([Routing before a tool](./writing-pipelines.md#routing-before-a-tool)), or the generated `route-tasks` [directive](./directives.md) tells it the command exists.

## Tasks

| Key | Meaning |
|---|---|
| `kind` | `summarize` · `digest` · `generate`, or a plugin kind |
| `upstream` | the `[upstream.*]` it calls. Without one the task is inert, every rule routing to it is inert too (W007), and `explain` says so |
| `temperature` | optional |
| `instructions` | optional — the worker's system prompt; default is the kind's |
| `prompt` | optional — assembly template: `${question}` `${files}` / `${spec}` `${reference}` |
| `context` | optional, default `false` — when RTK runs the task itself (a `route` after a tool, or a route with `mode = "swap"` or `"answer"`), ask the worker a question built from what the agent is doing: the user's request, the agent's current step and its recent calls. See below |

The task name is what rules target and what the agent types. A name that collides with a built-in `rtk` command does not load (E014).

**Resolution:** `rtk bulk-read …` → `[task.bulk-read]` → kind (interface, template, output handling) → upstream (model, key) → call → output.

**The prompt:** system = the instance's `instructions` or the kind's default; user = the template with the agent's intent (`--question` / `--spec`) and the file contents RTK reads from disk. The files never enter the agent's context — that is the saving. `generate` refuses to run without `--reference`: without a file to match, the worker writes context-free code.

**The question when RTK runs the task.** When the agent types the command, its `--question` is the question. When RTK runs the task on the agent's behalf — a [route after a tool](./writing-pipelines.md#routing-after-a-tool), or a [route mode](./writing-pipelines.md#route-modes) `swap` or `answer` — nobody wrote one. With `context = true` the worker gets the agent's [call context](./conditions.md#call-context), so it keeps what the agent was looking for:

```text
Question: The user asked: fix the failing build
The agent's current step: find which module fails and why
Its recent tool calls: shell mvn -q package | read pom.xml
What in this does the agent need for its current step? Keep exact names, numbers, paths, line numbers and error lines.
```

Without it, the worker gets only the last line. The context costs a few hundred input tokens per call. On Claude Code the agent's current step can lag one step behind, because the transcript is written asynchronously; the user's request and the recent calls are always current.

**Personas, not kinds:** a "review" task is `[task.review] kind = "summarize" instructions = "…"`. New personas are three lines; new kinds are [plugins](./plugins.md).

## Upstreams

An upstream is a model endpoint, shared by tasks and [judges](./conditions.md#judges--conditions-on-meaning).

| Key | Meaning |
|---|---|
| `provider` | `anthropic` · `openai` · `google` for a task; a judge upstream is reached through its `base_url`, whatever its `provider` says |
| `model` | the model name |
| `base_url` | optional — any compatible endpoint |
| `api_key_env` | the environment variable holding the key. Omit it for the provider's conventional variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`), which `validate` names for you, or for a judge that needs no key. A judge on this computer (`127.0.0.1`, `localhost`, `[::1]`) whose variable is unset is called without a key; a hosted one is skipped until it is set. A literal `api_key = "…"` does not load (E015) |
| `api_key_command` | or run a command and take its stdout (keychain, vault) — the same shape as Claude Code's `apiKeyHelper`. It runs through `sh` on Linux/macOS and `cmd` on Windows, so a command written for Git Bash needs rewriting there, or an explicit `bash -c "…"`. An upstream with one is never reported as needing auth |
| `timeout_secs` | default 30; set it lower for anything on the hook path |
| `dependency` | the [`[dependency.<name>]`](#dependencies) that serves this upstream on this computer; its `base_url` must point at the dependency's `listen` address (E022) |

`rtk pipeline validate`, `rtk trust` and `rtk init` print `auth needed: export VAR=...` for any key that is unset.

**Already-authenticated agents need no second key.** When the [RTK Proxy](../proxy/overview.md) is running and the upstream names a provider it speaks (`anthropic`, `openai`), a worker with no credential of its own is routed through the daemon, which lends the credential the agent is already relaying. An agent set up with `rtk init` delegates with nothing further to configure. A request that brings its own key is never substituted; with no daemon up the worker goes direct and a missing key is still an error. To refuse borrowing, set `RTK_PROXY_RELAY_BORROWING=false` on the daemon.

## Dependencies

A dependency is a service the pipeline needs on this computer, with the recipe to install and run it. RTK installs it only after the user says yes in a terminal, or when the user names a reviewed pipeline with `rtk init -g --pipeline <name>` where no terminal can ask ([how that works](./commands.md#what-a-pipeline-needs)). A pipeline whose dependency is refused is not installed. RTK starts the dependency when a call to the upstream that names it finds it stopped.

```toml
[upstream.judge]
provider   = "laya"
model      = "laya"
base_url   = "http://127.0.0.1:41733/v1/systemone"
dependency = "laya"

[dependency.laya]
about     = "Laya, an open-source decision model the judge asks"
needs     = "Node.js 20+, ~1.7 GB of model weights on first start, ~2 GB of RAM"
npm       = "@receptron/laya@0.2.0"
integrity = "sha512-C7eBcUUj/S4Mn2ccQoqflZghEi9OPu6RBUbSjcbnuPbQQAWTVFGxDRoLyyP59hm2t4oF8JbE+dP0IaUE58XsJQ=="
node      = 20
run       = ["node", "server.mjs"]
listen    = "127.0.0.1:41733"
health    = "/health"

[dependency.laya.files]
"server.mjs" = '''
// the server, shipped in the pipeline file
'''
```

| Key | Meaning |
|---|---|
| `npm` + `integrity` | an exact `name@version` and the sha512 npm publishes for it (`npm view <name>@<version> dist.integrity`). Uses the user's Node.js; `node` sets the lowest major version |
| `pip` | a list of `name==version --hash=sha256:<hex>` lines, installed into the dependency's own environment with `--require-hashes`, so every package, sub-dependencies included, must be listed with its hash. `python` sets the lowest version, like `"3.10"` |
| `binary` | one table per platform (`macos-arm64`, `macos-x64`, `linux-x64`, `linux-arm64`, `windows-x64`) with an https `url` and the file's `sha256`; a `.tar.gz`, `.tgz`, `.tar` or `.zip` is unpacked |
| `run` | the command that starts the service, program first. RTK looks for the program in the dependency's directory, its `bin/`, its Python environment, `node_modules/.bin`, then your `PATH` |
| `listen` | the local address it answers on: `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` |
| `health` | optional path RTK asks for status: 2xx is running, 503 is starting (a short plain-text body is shown as its progress) |
| `files` | small files written next to it before it runs, such as a server script |
| `env` | variables for running the service (not `PATH`). They are shown before the question, and never passed to npm, pip or the download |
| `about`, `needs` | what it is and what it needs, shown to the user before the question |

Exactly one installer per dependency. A recipe that is not pinned, or not local, does not load (E021). A key this rtk does not know (from a newer recipe format) loads with W026, and this rtk will not install that dependency. rtk 1.2 and older refuse a file with a `[dependency]` section, so set [`rtk_min`](./writing-pipelines.md#pipeline) on it. Dependency names are shared by every pipeline on the computer: two files that declare the same name with different recipes load with W028, and RTK refuses to install the second one.

**What people who install your pipeline need.** Before installing a pipeline, RTK checks what its file needs and does not install it when something is missing ([how that works](./commands.md#what-a-pipeline-needs)). As an author:

- Declare a `[dependency]` for every local service an upstream calls, and link it with `dependency = "<name>"`. An upstream at a loopback address with no linked dependency counts as a service the user must already be running: a user without it cannot install your pipeline, and RTK cannot offer to install it.
- Extending `rtk:proxy`, or writing `proxy.request` rules, makes the RTK daemon a need. A user who declines it, or has no `rtk-proxy`, does not get the pipeline.
- A hosted upstream needs only a key, and a key is never a need: RTK installs the pipeline and prints `auth needed` until the key is set.

## Cost

The worker's tokens are paid too. `rtk gain` shows `saved − worker cost`, net. Calls go directly to the provider, or through the RTK Proxy when it is running, in which case they are measured there.

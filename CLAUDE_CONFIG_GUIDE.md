# The `.claude/` Directory: A Technical Guide to Project-Level Claude Code Configuration

This guide documents the `.claude/` directory structure — the mechanism by which a project can deeply customize how Claude Code (Anthropic's CLI agent) behaves when working inside that repository. Every file in `.claude/` shapes Claude Code's decisions: what models it uses, what tools it can access, how it intercepts commands, and what quality standards it enforces.

This guide uses the [RTK (Rust Token Killer)](https://github.com/rtk-ai/rtk) repository as a real-world case study throughout.

---

## Table of Contents

- [1. What is `.claude/` and Why It Exists](#1-what-is-claude-and-why-it-exists)
- [2. Architecture Overview](#2-architecture-overview)
- [3. Component Deep Dives](#3-component-deep-dives)
  - [3.1 `settings.local.json` — Permission Control](#31-settingslocaljson--permission-control)
  - [3.2 `rules/` — Persistent Behavioral Instructions](#32-rules--persistent-behavioral-instructions)
  - [3.3 `agents/` — Specialized Sub-Agents](#33-agents--specialized-sub-agents)
  - [3.4 `hooks/` — Real-Time Command Interception](#34-hooks--real-time-command-interception)
  - [3.5 `commands/` — User-Invocable Slash Commands](#35-commands--user-invocable-slash-commands)
  - [3.6 `skills/` — Context-Aware Auto-Triggering Prompts](#36-skills--context-aware-auto-triggering-prompts)
  - [3.7 `worktrees/` — Isolated Agent Workspaces](#37-worktrees--isolated-agent-workspaces)
- [4. Runtime Interaction Model](#4-runtime-interaction-model)
- [5. Case Study: RTK Dogfooding](#5-case-study-rtk-dogfooding)
- [6. Patterns and Lessons](#6-patterns-and-lessons)

---

## 1. What is `.claude/` and Why It Exists

When Claude Code opens a project, it scans for a `.claude/` directory at the repository root. This directory acts as a **project-scoped configuration layer** that transforms a general-purpose AI coding assistant into a specialized, opinionated development partner.

Without `.claude/`, Claude Code operates with generic behavior: it reads files, runs commands, and follows its base instructions. With `.claude/`, the same agent can:

- Automatically rewrite shell commands before execution
- Spawn specialized sub-agents with constrained tools and models
- Enforce TDD workflows, pre-commit checks, and quality gates
- Run diagnostic scripts via slash commands
- Apply project-specific coding standards to every interaction

**Key distinction from `CLAUDE.md`**: The `CLAUDE.md` file (at the repo root) provides textual instructions that Claude Code reads as context. The `.claude/` directory provides **executable configuration** — hooks that run code, agents that spawn processes, commands that trigger workflows, and skills that the LLM auto-activates based on context. `CLAUDE.md` tells Claude what to think; `.claude/` tells Claude what to do.

---

## 2. Architecture Overview

```
.claude/
├── settings.local.json        # Permission overrides (auto-allow specific tools)
├── rules/                     # Persistent instructions loaded every conversation
│   └── cli-testing.md         # Testing strategy rules
├── agents/                    # Specialized sub-agents (model, tools, system prompt)
│   ├── rust-rtk.md            # Rust expert agent
│   ├── rtk-testing-specialist.md
│   ├── code-reviewer.md
│   ├── debugger.md
│   └── technical-writer.md
├── hooks/                     # Shell scripts that intercept tool calls in real-time
│   ├── rtk-rewrite.sh         # PreToolUse:Bash — auto-rewrites commands
│   ├── rtk-suggest.sh         # PreToolUse:Bash — suggests RTK equivalents
│   └── bash/
│       └── pre-commit-format.sh  # Pre-commit quality gate
├── commands/                  # User-invocable slash commands (/diagnose, etc.)
│   ├── diagnose.md
│   ├── test-routing.md
│   └── rtk_repo_explainer.md
├── skills/                    # Auto-triggering context-aware prompt templates
│   ├── rtk-tdd/               # TDD workflow (with reference docs)
│   │   ├── SKILL.md
│   │   └── references/
│   ├── issue-triage/          # GitHub issue management
│   │   ├── SKILL.md
│   │   └── templates/
│   ├── pr-triage/             # GitHub PR review
│   │   ├── SKILL.md
│   │   └── templates/
│   ├── performance.md         # Performance optimization workflow
│   ├── security-guardian.md   # Security analysis workflow
│   ├── ship.md                # Release automation workflow
│   └── repo-recap.md          # Repository status report generator
└── worktrees/                 # Isolated git worktrees for parallel agent work
    └── eloquent-brattain/     # Leftover from a previous agent session
```

**Information flow at runtime:**

```
User types a message or command
         │
         ▼
┌─────────────────────┐
│   rules/ loaded as  │  ← Always active, every conversation
│   system context     │
└────────┬────────────┘
         │
         ▼
┌─────────────────────┐
│  Claude Code main   │  ← Reads CLAUDE.md + rules/ + settings
│  conversation loop  │
└────────┬────────────┘
         │
    ┌────┴────┬──────────┬──────────────┐
    ▼         ▼          ▼              ▼
 hooks/    agents/    commands/      skills/
 (intercept (spawn     (user types   (auto-trigger
  tool       sub-       /diagnose)    on context
  calls)     processes)               match)
```

---

## 3. Component Deep Dives

### 3.1 `settings.local.json` — Permission Control

**File**: `.claude/settings.local.json`

Claude Code has a permission system — every tool call (reading files, running bash commands, editing code) can be auto-allowed, prompted, or denied. This file configures project-level permission overrides.

**Structure:**

```json
{
  "permissions": {
    "allow": [
      "Read",
      "Bash(ls -la /c/dojo/Rust/github/rtk/src/*.rs)",
      "Bash(wc:*)",
      "Bash(find:*)"
    ]
  }
}
```

**How it works:**

| Entry | Effect |
|-------|--------|
| `"Read"` | Auto-allows all file reads without prompting the user |
| `"Bash(ls -la /path/*.rs)"` | Auto-allows this exact `ls` command |
| `"Bash(wc:*)"` | Auto-allows any `wc` command (wildcard on arguments) |
| `"Bash(find:*)"` | Auto-allows any `find` command |

**Why it matters:** Without permission overrides, Claude Code would prompt the user for approval on every file read or shell command. For a project where the agent frequently reads source files and runs build tools, this eliminates friction. The tradeoff is explicit: the project author has decided these specific operations are safe to auto-approve.

**Scope:** This file is `.local` — it is typically gitignored and applies only to the current developer's machine. A shared `settings.json` (without `.local`) can be committed to the repository for team-wide defaults.

---

### 3.2 `rules/` — Persistent Behavioral Instructions

**Directory**: `.claude/rules/`

Rules are markdown files that get loaded into Claude Code's system context at the start of every conversation. They act as persistent instructions that shape behavior across all interactions within the project.

**Example: `cli-testing.md`**

This 450+ line file defines RTK's complete testing strategy:

- **Snapshot testing** — Mandates use of the `insta` crate for output validation
- **Token accuracy testing** — Requires all filters to verify 60-90% token savings with real fixtures
- **Cross-platform testing** — Defines platform-specific shell escaping rules
- **Integration testing** — Specifies real command execution tests
- **Performance testing** — Sets <10ms startup time and <5MB memory targets

**Key characteristics:**

| Property | Detail |
|----------|--------|
| **Loading** | Automatic, every conversation |
| **Format** | Markdown with optional YAML frontmatter |
| **Scope** | Behavioral — shapes *how* Claude works, not *what* it does |
| **Persistence** | Always active, no user action required |

**Difference from CLAUDE.md:** Rules in `.claude/rules/` are modular — you can have separate files for testing strategy, coding standards, security policy, etc. `CLAUDE.md` is a single monolithic file. Both are loaded as context, but rules allow better organization.

**When to use rules:**
- Coding standards that must always apply (formatting, error handling patterns)
- Testing requirements (coverage thresholds, fixture strategies)
- Architecture constraints (no async, lazy regex only)
- Quality gates (what must pass before committing)

---

### 3.3 `agents/` — Specialized Sub-Agents

**Directory**: `.claude/agents/`

Agent definitions are markdown files with YAML frontmatter that configure specialized sub-agents. When Claude Code encounters a task matching an agent's domain, it can spawn that agent as an isolated subprocess with its own model, tool access, and system prompt.

**Anatomy of an agent definition:**

```markdown
---
name: rust-rtk
description: Expert Rust developer for RTK - CLI proxy patterns, filter design, performance optimization
model: claude-sonnet-4-5-20250929
tools: Read, Write, Edit, MultiEdit, Bash, Grep, Glob
---

# Rust Expert for RTK

You are an expert Rust developer specializing in the RTK codebase architecture.

## Core Responsibilities
- CLI proxy architecture: Command routing, stdin/stdout forwarding...
- Filter development: Regex-based condensation, token counting...
...
```

**Frontmatter fields:**

| Field | Purpose | Example |
|-------|---------|---------|
| `name` | Identifier for the agent | `rust-rtk` |
| `description` | When to use this agent (shown to Claude Code for routing) | Long description with examples |
| `model` | Which Claude model to use | `claude-sonnet-4-5-20250929`, `sonnet`, `haiku` |
| `tools` | Allowed tool set (restricts what the agent can do) | `Read, Write, Edit, Bash, Grep, Glob` |
| `color` | Terminal UI color for the agent's output | `red` |
| `permissionMode` | Override permission mode | `ask` |
| `disallowedTools` | Explicitly block specific tools | `Write, Edit` (read-only agent) |
| `skills` | Skills the agent can use | `security-guardian, backend-architect` |

**RTK's agent lineup:**

| Agent | Model | Tools | Purpose |
|-------|-------|-------|---------|
| **rust-rtk** | Sonnet | Read, Write, Edit, MultiEdit, Bash, Grep, Glob | Rust development — filter design, performance optimization |
| **rtk-testing-specialist** | Sonnet | Read, Write, Edit, Bash, Grep, Glob | Snapshot tests, token accuracy, cross-platform validation |
| **code-reviewer** | Sonnet | (default + skills) | Security audit, performance analysis, code quality |
| **debugger** | Sonnet | Read-only (Write/Edit disallowed) | Root cause analysis without making changes |
| **technical-writer** | Sonnet | Read, Write, Edit, Bash | Documentation creation |

**Design insight — the read-only debugger:**

The `debugger` agent has `disallowedTools: [Write, Edit]` and `permissionMode: ask`. This is a deliberate architectural choice: the debugger can investigate, read code, run diagnostics, and produce a root cause analysis — but it cannot modify any files. This prevents a debugging session from accidentally introducing new bugs. The fix is applied by the main conversation or a different agent after reviewing the debugger's findings.

**Design insight — proactive agents:**

The `code-reviewer` agent has an extensive `description` field with multiple `<example>` blocks showing when Claude Code should spawn it. This description isn't just documentation — Claude Code reads it to decide when to proactively invoke the agent. The examples teach Claude to automatically trigger a code review after the user finishes implementing a filter, without being asked.

---

### 3.4 `hooks/` — Real-Time Command Interception

**Directory**: `.claude/hooks/`

Hooks are the most powerful component in `.claude/`. They are executable shell scripts that Claude Code runs in response to lifecycle events — specifically, **before** or **after** tool calls. A hook can inspect, modify, block, or augment any tool invocation.

**Hook event model:**

```
User message → Claude decides to use Bash tool
                        │
                        ▼
              ┌─────────────────┐
              │  PreToolUse:Bash │ ← Hook receives JSON with tool_input
              │  (rtk-rewrite.sh)│
              └────────┬────────┘
                       │
              ┌────────┴────────┐
              │  Hook can:       │
              │  • Modify input  │ ← Change the command before execution
              │  • Allow/deny    │ ← Block the tool call entirely
              │  • Add messages  │ ← Inject system messages
              │  • Pass through  │ ← Do nothing (exit 0)
              └────────┬────────┘
                       │
                       ▼
              Tool executes (with possibly modified input)
```

#### 3.4.1 `rtk-rewrite.sh` — The Auto-Rewriter

**Event**: `PreToolUse:Bash`
**Purpose**: Transparently rewrites raw commands to their RTK equivalents before Claude Code executes them.

**How it works, step by step:**

1. Claude Code is about to run a Bash command (e.g., `git status`)
2. The hook receives a JSON payload on stdin:
   ```json
   {
     "tool_input": {
       "command": "git status",
       "description": "Show working tree status"
     }
   }
   ```
3. The hook checks guards:
   - Is `rtk` binary available? Is `jq` available? If not, exit silently.
   - Is the command empty? Skip.
   - Is it a heredoc (`<<`)? Skip (can't safely rewrite multi-line commands).
4. It calls `rtk rewrite "git status"` — RTK itself is the single source of truth for command mappings
5. If RTK returns a rewritten command (`rtk git status`), the hook outputs:
   ```json
   {
     "hookSpecificOutput": {
       "hookEventName": "PreToolUse",
       "permissionDecision": "allow",
       "permissionDecisionReason": "RTK auto-rewrite",
       "updatedInput": {
         "command": "rtk git status",
         "description": "Show working tree status"
       }
     }
   }
   ```
6. Claude Code executes `rtk git status` instead of `git status`
7. The user sees token-optimized output without any manual intervention

**Key design decisions:**

- **Single source of truth**: The hook does not contain a mapping table. It delegates to `rtk rewrite`, which means adding new command support requires only updating RTK's Rust code — not editing shell scripts.
- **Audit logging**: Opt-in via `RTK_HOOK_AUDIT=1`. Logs every hook decision (skip, rewrite, pass-through) to `~/.local/share/rtk/hook-audit.log` with timestamps.
- **Silent failure**: If `rtk` or `jq` is missing, the hook exits silently (exit 0). It never blocks the user's command.
- **Idempotent**: If the command is already using RTK (`rtk git status`), the hook detects identical input/output and skips.

#### 3.4.2 `rtk-suggest.sh` — The Suggestion Hook

**Event**: `PreToolUse:Bash`
**Purpose**: Instead of rewriting commands, this hook emits a system message suggesting the RTK equivalent. Claude Code sees the suggestion and can decide whether to use it.

**Output format:**

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "systemMessage": "RTK available: `rtk git status` (60-90% token savings)"
  }
}
```

**Difference from rtk-rewrite.sh**: The suggest hook is non-invasive. It allows the original command to run unchanged and merely informs Claude that an RTK alternative exists. This is useful during development or testing when you want full raw output but still want Claude to learn about RTK options.

**Command coverage** (the suggest hook has explicit pattern matching for):

| Category | Commands |
|----------|----------|
| Git | `status`, `diff`, `log`, `add`, `commit`, `push`, `pull`, `branch`, `fetch`, `stash`, `show` |
| GitHub CLI | `gh pr`, `gh issue`, `gh run` |
| Cargo | `test`, `build`, `clippy`, `check`, `install`, `nextest`, `fmt` |
| File operations | `cat` (to `rtk read`), `grep`/`rg`, `ls`, `tree`, `find`, `diff`, `head` |
| JS/TS tooling | `vitest`, `tsc`, `eslint`, `prettier`, `playwright`, `prisma` |
| Containers | `docker ps/images/logs`, `kubectl get/logs` |
| Network | `curl`, `wget` |
| Package managers | `pnpm list/ls/outdated` |

#### 3.4.3 `bash/pre-commit-format.sh` — Pre-Commit Quality Gate

**Purpose**: Runs `cargo fmt` and `cargo clippy` before any git commit Claude Code attempts.

```bash
#!/bin/bash
echo "Running Rust pre-commit checks..."
cargo fmt --all
if cargo clippy --all-targets 2>&1 | grep -q "error:"; then
    echo "Clippy found errors. Fix them before committing."
    exit 1
fi
echo "Pre-commit checks passed (warnings allowed)"
```

**Behavior**: Auto-formats code, then blocks the commit if clippy finds errors (warnings are allowed). This ensures every commit Claude Code makes passes basic quality gates regardless of what instructions the user gives.

---

### 3.5 `commands/` — User-Invocable Slash Commands

**Directory**: `.claude/commands/`

Commands are markdown files that define slash commands users can type in a Claude Code session. When the user types `/diagnose`, Claude Code reads the corresponding `.md` file and executes the workflow it describes.

**Anatomy of a command:**

```markdown
---
model: haiku
description: RTK environment diagnostics - Checks installation, hooks, version, command routing
---

# /diagnose

[Workflow steps, bash commands to run, output format, suggested fixes...]
```

**Frontmatter fields:**

| Field | Purpose |
|-------|---------|
| `model` | Which Claude model handles this command (e.g., `haiku` for fast/cheap diagnostics) |
| `description` | One-line description shown in command listings |

**RTK's command lineup:**

| Command | Model | Purpose |
|---------|-------|---------|
| `/diagnose` | Haiku | Full environment health check — RTK installation, hook permissions, version compatibility, token analytics |
| `/test-routing` | Haiku | Dry-run command routing — checks if a command has an RTK filter without executing anything |
| `/rtk_repo_explainer` | (default) | Deep architecture walkthrough — 9-step codebase onboarding session |

**Design insight — model selection for commands:**

`/diagnose` and `/test-routing` use the `haiku` model — the fastest and cheapest Claude variant. This makes sense because these commands run scripted checks (bash commands, grep patterns) that don't require deep reasoning. The cost of using Opus or Sonnet for "check if a file exists and print a status" would be wasteful.

In contrast, `/rtk_repo_explainer` uses the default model (typically the most capable one in the conversation) because it requires deep code analysis, architectural reasoning, and pedagogical explanation.

**Design insight — the `/diagnose` command as self-healing:**

The `/diagnose` command doesn't just report problems — it proposes interactive fixes via `AskUserQuestion` with multi-select options:

```
question: "Problems detected. Which fixes to apply?"
options:
  - "cargo install --path ." (Install RTK locally)
  - "chmod +x .claude/hooks/*.sh" (Fix hook permissions)
  - "Fix all (recommended)"
```

This turns a diagnostic tool into a self-healing workflow. Claude Code can detect that RTK isn't installed, offer to install it, and verify the fix — all within one interaction.

---

### 3.6 `skills/` — Context-Aware Auto-Triggering Prompts

**Directory**: `.claude/skills/`

Skills are prompt templates that can be invoked explicitly (via `/skill-name`) or auto-triggered when Claude Code detects a matching context. They are more structured than rules — a skill defines a complete workflow with steps, outputs, and triggers.

**Anatomy of a skill:**

```markdown
---
name: rtk-tdd
description: >
  Enforces TDD (Red-Green-Refactor) for Rust development. Auto-triggers on
  implementation, testing, refactoring, and bug fixing tasks.
---

# Rust TDD Workflow

## Three Laws of TDD
1. Do NOT write production code without a failing test
2. Write only enough test to fail
3. Write only enough production code to pass

Cycle: RED (test fails) -> GREEN (minimum to pass) -> REFACTOR (cleanup)
...
```

**Skill vs. Command vs. Rule:**

| Aspect | Rule | Command | Skill |
|--------|------|---------|-------|
| **Trigger** | Always loaded | User types `/name` | User types `/name` or context match |
| **Scope** | Behavioral guidelines | One-off workflow | Reusable workflow template |
| **Structure** | Free-form markdown | Steps + bash commands | Steps + templates + references |
| **Subdirectories** | No | No | Yes (templates/, references/) |
| **Example** | "Always use lazy_static for regex" | "/diagnose" runs health check | "/rtk-tdd" enforces Red-Green-Refactor |

**RTK's skill lineup:**

| Skill | Trigger | Purpose |
|-------|---------|---------|
| **rtk-tdd** | Implementation, testing, refactoring tasks | Enforces TDD cycle: write failing test, implement minimum, refactor |
| **performance** | Filter changes, dependency additions, release prep | Systematic benchmarking: startup time, memory, token savings, binary size |
| **security-guardian** | Filter changes, shell execution logic, hook modifications | Threat modeling: command injection, shell escaping, hook security, malicious output |
| **ship** | Release preparation | Full release workflow: version bump, changelog, tag, push, CI verification |
| **repo-recap** | Team status reporting | Generates Markdown recap of PRs, issues, releases (supports FR/EN) |
| **issue-triage** | >10 open issues, stale issues >30 days | Audit, categorize, detect duplicates, cross-reference PRs, post comments |
| **pr-triage** | >5 open PRs without review, stale PRs >14 days | Audit, deep review, draft and post review comments |

**Design insight — skills with subdirectories:**

Complex skills like `issue-triage` and `pr-triage` include `templates/` subdirectories containing comment templates for GitHub. The `rtk-tdd` skill includes `references/testing-patterns.md` with RTK-specific test patterns and an untested module backlog. This allows skills to carry reference material without bloating the main SKILL.md file.

**Design insight — proactive triggering:**

Skills like `security-guardian` declare: *"Automatically triggered: After filter changes, shell command execution logic, hook modifications."* This means Claude Code should invoke the security analysis workflow without being asked whenever it detects relevant code changes. The skill essentially installs a behavioral reflex into the agent.

**Design insight — bilingual operation:**

The `repo-recap`, `issue-triage`, and `pr-triage` skills support French (default) and English output via arguments. However, GitHub comments are always posted in English regardless of the display language — a pragmatic choice for an international audience.

---

### 3.7 `worktrees/` — Isolated Agent Workspaces

**Directory**: `.claude/worktrees/`

When Claude Code spawns an agent with `isolation: "worktree"`, it creates a temporary git worktree — an isolated copy of the repository where the agent can make changes without affecting the main working directory.

**How it works:**

```
Main repo (user's working directory)
    │
    ├── .claude/worktrees/
    │   └── eloquent-brattain/    ← Isolated copy of the full repo
    │       ├── src/              ← Agent can freely edit these files
    │       ├── Cargo.toml
    │       └── ...
    │
    └── src/                      ← User's files are untouched
```

**Lifecycle:**

1. Agent is spawned with `isolation: "worktree"`
2. Git creates a worktree branch (auto-named, e.g., `eloquent-brattain`)
3. Agent works in the isolated copy — reads, writes, runs tests
4. When done:
   - If **no changes** were made: worktree is automatically cleaned up
   - If **changes** were made: worktree path and branch name are returned to the caller

**Use cases:**

- **Parallel development**: Multiple agents working on different features simultaneously
- **Experimental changes**: Try an approach without risking the main working tree
- **Code review**: An agent can check out a PR branch in a worktree and review it independently

**Note**: The `eloquent-brattain` worktree in this repository is a leftover from a previous agent session that made changes. It contains a full snapshot of the repository at the time the agent ran — including newer files not yet in the main branch (like `src/parser/`, `src/learn/`, `src/filters/*.toml`).

---

## 4. Runtime Interaction Model

Here's how all components compose during a typical Claude Code session:

### Scenario: User asks Claude to "add a new filter for `dotnet build`"

```
Step 1: CONTEXT LOADING
├── CLAUDE.md loaded (project instructions)
├── .claude/rules/cli-testing.md loaded (testing requirements)
└── settings.local.json applied (auto-allow Read, specific Bash commands)

Step 2: SKILL ACTIVATION
├── rtk-tdd skill auto-triggers (implementation task detected)
│   └── Enforces: write test first, then implement, then refactor
└── security-guardian skill queued (filter change = security review needed)

Step 3: AGENT SPAWNING
├── Claude may spawn rust-rtk agent for implementation
│   ├── Model: Sonnet (fast, capable enough for filter code)
│   ├── Tools: Read, Write, Edit, Bash, Grep, Glob
│   └── System prompt: RTK patterns, lazy_static, fallback design
└── After implementation, code-reviewer agent spawned proactively
    ├── Model: Sonnet
    └── Checks: lazy regex, fallback, token savings, exit codes

Step 4: HOOK INTERCEPTION (during development)
├── Claude runs `cargo test` → rtk-rewrite.sh intercepts
│   └── Rewrites to: `rtk cargo test` (token-optimized output)
├── Claude runs `cargo clippy` → rtk-rewrite.sh intercepts
│   └── Rewrites to: `rtk cargo clippy`
└── Claude runs `git commit` → pre-commit-format.sh intercepts
    └── Runs: cargo fmt + clippy error check before allowing commit

Step 5: QUALITY VERIFICATION
├── rtk-testing-specialist agent verifies snapshot tests
├── performance skill checks startup time < 10ms
└── security-guardian skill checks for command injection vectors
```

### Scenario: User types `/diagnose`

```
Step 1: COMMAND DISPATCH
├── Claude Code recognizes /diagnose as a command
├── Reads .claude/commands/diagnose.md
└── Uses haiku model (fast, scripted checks)

Step 2: PARALLEL CHECKS
├── which rtk && rtk --version
├── git status --short && git branch
├── Check hook file existence and permissions
├── Check Claude Code environment variables
└── Test command routing (rtk --help | grep gain)

Step 3: REPORTING
├── Formats status dashboard with pass/fail indicators
└── If problems found: offers interactive fix options

Step 4: SELF-HEALING (if user approves)
├── cargo install --path .
├── chmod +x .claude/hooks/*.sh
└── Verification: rtk --version && rtk gain
```

---

## 5. Case Study: RTK Dogfooding

RTK is a CLI proxy that filters command outputs to save LLM tokens. Its `.claude/` configuration is a textbook example of **dogfooding** — the project uses its own tool to optimize its own development workflow.

### The Feedback Loop

```
Developer writes RTK code
    │
    ▼
Claude Code helps via .claude/ configuration
    │
    ▼
Hooks rewrite commands through RTK ← RTK filters its own build output
    │
    ▼
Token savings tracked by RTK ← RTK measures its own impact
    │
    ▼
`rtk gain` shows savings ← Developer sees the value
    │
    ▼
Developer improves RTK ← Cycle repeats
```

### Concrete examples of dogfooding

**1. Every `cargo test` Claude runs goes through RTK:**

When Claude runs `cargo test` during development, the `rtk-rewrite.sh` hook transparently rewrites it to `rtk cargo test`. RTK's cargo test filter shows only failures (90%+ token reduction). Claude sees condensed output, uses fewer tokens, and the savings are tracked in RTK's own database.

**2. RTK's hook uses RTK's own `rewrite` command:**

The `rtk-rewrite.sh` hook calls `rtk rewrite "$CMD"` to determine the rewritten command. This means RTK's command mapping logic (in `src/discover/registry.rs`) is exercised every time any hook fires — constant real-world testing.

**3. The `/diagnose` command checks RTK's own health:**

The diagnostic workflow verifies that RTK is installed, hooks are executable, and `rtk gain` works — all tools that RTK itself provides. If anything breaks, the developer finds out immediately during their normal workflow.

---

## 6. Patterns and Lessons

### Pattern 1: Layered Enforcement

RTK uses multiple layers to enforce its quality standards:

| Layer | Mechanism | When |
|-------|-----------|------|
| **Rules** | `cli-testing.md` | Every conversation (passive context) |
| **Skills** | `rtk-tdd` | Every implementation task (active workflow) |
| **Hooks** | `pre-commit-format.sh` | Every commit (hard gate) |
| **Agents** | `code-reviewer` | After implementation (proactive review) |

Each layer catches different types of issues. Rules remind Claude of standards. Skills enforce workflows. Hooks block bad commits. Agents catch subtle problems. Together, they form defense in depth.

### Pattern 2: Model Selection by Task Complexity

Not every task needs the most capable model:

| Task Type | Model Choice | Rationale |
|-----------|-------------|-----------|
| Environment diagnostics | Haiku | Scripted checks, no reasoning needed |
| Filter implementation | Sonnet | Code generation, pattern matching |
| Code review | Sonnet | Analysis, but structured by checklist |
| Architecture explanation | Opus/default | Deep reasoning, pedagogy |
| Debugging (read-only) | Sonnet | Investigation without write access |

This saves cost and latency. A `/diagnose` command that just runs `which rtk` doesn't need Opus-level reasoning.

### Pattern 3: Separation of Observation and Action

The debugger agent can only read, not write. The suggest hook informs but doesn't modify. This separation ensures that analysis and modification are distinct steps with different approval requirements. The human (or the main conversation agent) reviews findings before changes are applied.

### Pattern 4: Single Source of Truth

The `rtk-rewrite.sh` hook does not duplicate RTK's command mapping. It calls `rtk rewrite` and delegates the decision. The suggest hook (`rtk-suggest.sh`) has its own pattern matching — a pragmatic tradeoff for cases where `rtk rewrite` isn't available. When maintaining mapping logic, there's one canonical location: RTK's Rust source code.

### Pattern 5: Graceful Degradation Everywhere

Every hook, agent, and skill is designed to fail silently:

- Hook can't find `rtk` binary? Exit 0 (command runs unmodified).
- Filter crashes on malformed input? Fall back to raw command execution.
- Agent model unavailable? Main conversation handles the task.
- Skill doesn't match context? It simply doesn't activate.

No component in `.claude/` should ever block the developer's workflow. The worst case is always "behaves like vanilla Claude Code."

### Pattern 6: Audit Trail

The rewrite hook supports opt-in audit logging (`RTK_HOOK_AUDIT=1`) that records every decision:

```
2026-03-15T10:23:01Z | rewrite | git status | rtk git status
2026-03-15T10:23:05Z | skip:already_rtk | rtk cargo test | -
2026-03-15T10:23:12Z | skip:heredoc | cat <<EOF... | -
2026-03-15T10:23:18Z | skip:no_match | echo "hello" | -
```

This is invaluable for debugging hook behavior and understanding what RTK intercepts in practice.

---

## Summary

The `.claude/` directory transforms Claude Code from a generic assistant into a project-specific development partner. Its power comes from composing simple primitives:

| Primitive | What It Does | Analogy |
|-----------|-------------|---------|
| **settings** | Controls permissions | `.editorconfig` for AI tool access |
| **rules** | Shapes behavior passively | Linting rules, always on |
| **agents** | Spawns specialized workers | Microservices with bounded contexts |
| **hooks** | Intercepts and modifies actions | Git hooks / middleware |
| **commands** | Defines user-triggered workflows | Makefile targets / npm scripts |
| **skills** | Auto-triggers contextual workflows | IDE code actions / quick fixes |
| **worktrees** | Isolates parallel work | Docker containers for code |

Together, these components let a project author encode their development philosophy — quality gates, testing strategies, security policies, performance targets — directly into the AI agent that works on their code. The agent doesn't just follow instructions; it enforces them through hooks, validates them through agents, and maintains them through skills.

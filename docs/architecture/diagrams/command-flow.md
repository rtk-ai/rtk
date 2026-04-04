---
title: Command Flow
description: End-to-end diagram of how a command flows through RTK from agent to LLM
sidebar:
  order: 1
---

# Command Flow

End-to-end flow from the AI agent issuing a command to the filtered output reaching the LLM.

## With hook (transparent rewrite)

```mermaid
flowchart TD
    A["AI Agent\n(Claude Code, Cursor, etc.)"] -->|"runs: cargo test"| B

    subgraph HOOK ["Hook Interception (PreToolUse)"]
        B["Hook reads JSON input\nextract command string"] --> C
        C["rtk rewrite 'cargo test'"] --> D
        D{"Registry match?"}
        D -->|"yes"| E["returns 'rtk cargo test'"]
        D -->|"no match"| F["returns original unchanged"]
    end

    E --> G
    F --> G

    subgraph RTK ["RTK Binary"]
        G["Phase 1: Parse\nClap → Commands::Cargo"] --> H
        H["Phase 2: Route\ncargo::run(args)"] --> I
        I["Phase 3: Execute\nstd::process::Command::new('cargo')\n.args(['test'])"] --> J
        J["Phase 4: Filter\nfailures only\n200 lines → 5 lines"] --> K
        K["Phase 5: Print\nprintln!(filtered)"] --> L
        L["Phase 6: Track\nSQLite INSERT\n(input=5000tok, output=50tok)"]
    end

    K -->|"filtered output"| M["LLM Context\n~90% fewer tokens"]
```

## Without hook (direct usage)

```mermaid
flowchart LR
    A["Developer\ntype: rtk git status"] --> B["RTK Binary"]
    B --> C["git status (subprocess)"]
    C -->|"20 lines raw"| B
    B -->|"5 lines filtered"| D["Terminal\n(or LLM context)"]
```

## Filter lookup (TOML path)

```mermaid
flowchart LR
    CMD["rtk my-tool args"] --> P1
    P1{"1. .rtk/filters.toml\n(project-local)"}
    P1 -->|"match"| WIN["apply filter → print"]
    P1 -->|"no match"| P2
    P2{"2. ~/.config/rtk/filters.toml\n(user-global)"}
    P2 -->|"match"| WIN
    P2 -->|"no match"| P3
    P3{"3. BUILTIN_TOML\n(binary)"}
    P3 -->|"match"| WIN
    P3 -->|"no match"| P4[["exec raw\n(passthrough)"]]
```

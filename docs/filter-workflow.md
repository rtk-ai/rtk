# How a TOML filter goes from file to execution

This document explains what happens between "I created `src/filters/my-tool.toml`" and "RTK filters the output of `my-tool`".

## Build pipeline

```mermaid
flowchart TD
    A[["📄 src/filters/my-tool.toml\n(new file)"]] --> B

    subgraph BUILD ["🔨 cargo build"]
        B["build.rs\n① ls src/filters/*.toml\n② sort alphabetically\n③ concat → schema_version = 1 + all files"] --> C
        C{"TOML valid?\nDuplicate names?"} -->|"❌ panic! (build fails)"| D[["🛑 Error message\npoints to bad file"]]
        C -->|"✅ ok"| E[["OUT_DIR/builtin_filters.toml\n(generated file)"]]
        E --> F["rustc\ninclude_str!(concat!(env!(OUT_DIR),\n'/builtin_filters.toml'))"]
        F --> G[["🦀 rtk binary\nBUILTIN_TOML embedded"]]
    end

    subgraph TESTS ["🧪 cargo test"]
        H["test_builtin_filter_count\nassert_eq!(filters.len(), N)"] -->|"❌ count wrong"| I[["FAIL\n'Expected N, got N+1'\nUpdate the count'"]]
        J["test_builtin_all_expected_\nfilters_present\nassert!(names.contains('my-tool'))"] -->|"❌ name missing"| K[["FAIL\n'my-tool is missing—\nwas its .toml deleted?'"]]
        L["test_builtin_all_filters_\nhave_inline_tests\nassert!(tested.contains(name))"] -->|"❌ no tests"| M[["FAIL\n'Add tests.my-tool\nentries'"]]
    end

    subgraph VERIFY ["✅ rtk verify"]
        N["runs [[tests.my-tool]]\ninput → filter → compare expected"]
        N -->|"❌ mismatch"| O[["FAIL\nshows actual vs expected"]]
        N -->|"✅ pass"| P[["60/60 tests passed"]]
    end

    G --> H
    G --> J
    G --> L
    G --> N

    subgraph RUNTIME ["⚡ rtk my-tool --verbose"]
        Q["Claude Code hook\nmy-tool ... → rtk my-tool ..."] --> R
        R["TomlFilterRegistry::load()\n① .rtk/filters.toml  (project)\n② ~/.config/rtk/filters.toml  (user)\n③ BUILTIN_TOML  (binary)\n④ passthrough"] --> S
        S{"match_command\n'^my-tool\\b'\nmatches?"} -->|"No match"| T[["exec raw\n(passthrough)"]]
        S -->|"✅ match"| U["exec command\ncapture stdout"]
        U --> V

        subgraph PIPELINE ["8-stage filter pipeline"]
            V["strip_ansi"] --> W["replace"]
            W --> X{"match_output\nshort-circuit?"}
            X -->|"✅ pattern matched"| Y[["emit message\nstop pipeline"]]
            X -->|"no match"| Z["strip/keep_lines"]
            Z --> AA["truncate_lines_at"]
            AA --> AB["tail_lines"]
            AB --> AC["max_lines"]
            AC --> AD{"output\nempty?"}
            AD -->|"yes"| AE[["emit on_empty"]]
            AD -->|"no"| AF[["print filtered\noutput + exit code"]]
        end
    end

    G --> Q

    style BUILD fill:#1e3a5f,color:#fff
    style TESTS fill:#1a3a1a,color:#fff
    style VERIFY fill:#2d1b69,color:#fff
    style RUNTIME fill:#3a1a1a,color:#fff
    style PIPELINE fill:#4a2a00,color:#fff
    style D fill:#8b0000,color:#fff
    style I fill:#8b0000,color:#fff
    style K fill:#8b0000,color:#fff
    style M fill:#8b0000,color:#fff
    style O fill:#8b0000,color:#fff
```

## Step-by-step summary

| Step | Who | What happens | Fails if |
|------|-----|--------------|----------|
| 1 | Contributor | Creates `src/filters/my-tool.toml` | — |
| 2 | `build.rs` | Concatenates all `.toml` files alphabetically | TOML syntax error, duplicate filter name |
| 3 | `rustc` | Embeds result in binary via `BUILTIN_TOML` const | — |
| 4 | `cargo test` | 3 guards check count, names, inline test presence | Count not updated, name not in list, no `[[tests.*]]` |
| 5 | `rtk verify` | Runs each `[[tests.my-tool]]` entry | Filter logic doesn't match expected output |
| 6 | Runtime | Hook rewrites command, registry looks up filter, pipeline runs | No match → passthrough (not an error) |

## Filter lookup priority at runtime

```mermaid
flowchart LR
    CMD["rtk my-tool args"] --> P1
    P1{"1. .rtk/filters.toml\n(project-local)"}
    P1 -->|"✅ match"| WIN["apply filter"]
    P1 -->|"no match"| P2
    P2{"2. ~/.config/rtk/filters.toml\n(user-global)\n(macOS alt: ~/Library/Application Support/rtk/filters.toml)"}
    P2 -->|"✅ match"| WIN
    P2 -->|"no match"| P3
    P3{"3. BUILTIN_TOML\n(binary)"}
    P3 -->|"✅ match"| WIN
    P3 -->|"no match"| P4[["exec raw\n(passthrough)"]]
```

First match wins. A project filter with the same name as a built-in shadows the built-in and triggers a warning:

```
[rtk] warning: filter 'make' is shadowing a built-in filter
```

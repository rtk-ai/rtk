## graphify — MANDATORY FOR ROOT CAUSE ANALYSIS

This project has a knowledge graph at `graphify-out/` with god nodes, community structure, and cross-file relationships.

**CRITICAL: You MUST use Graphify for ALL root cause analysis, debugging, and codebase understanding tasks. Do NOT grep, read files directly, or search manually — query the graph instead.**

### Enforcement Rules

1. **Before ANY codebase question**: Run `graphify query "<question>"` — this returns a scoped subgraph (typically <2000 tokens) far more efficient than reading files.

2. **For root cause / relationship tracing**: Run `graphify path "A" "B"` to trace exact connections between components.

3. **For concept explanation**: Run `graphify explain "<concept>"` to get node + neighbors with confidence tags (EXTRACTED vs INFERRED).

4. **If graph is missing or stale**: Run `graphify update .` (AST-only, no API cost, takes seconds). This is your responsibility — do not skip.

5. **If Graphify is not installed**: Install it globally once per machine, then use it:
   ```bash
   # One-time global setup (run once per machine)
   python3.12 -m venv ~/.graphify-venv
   ~/.graphify-venv/bin/pip install graphifyy
   ~/.graphify-venv/bin/graphify hook install  # installs git hooks for auto-update
   ```
   Then in any repo: `~/.graphify-venv/bin/graphify extract . --code-only && ~/.graphify-venv/bin/graphify cluster-only .`

6. **Global availability**: The venv at `~/.graphify-venv/` is shared across ALL projects. No per-project reinstall needed.

7. **Graph outputs to use**:
   - `graphify-out/graph.json` — queryable graph (primary)
   - `graphify-out/GRAPH_REPORT.md` — architecture summary, god nodes, communities
   - `graphify-out/graph.html` — interactive visual (open in browser)
   - `graphify-out/wiki/` — markdown wiki (run `graphify export wiki`)

8. **Strict prohibition**: Do NOT use `grep`, `rg`, `find`, `cat`, `read` tools for codebase exploration when the graph exists. The graph has AST-resolved cross-file edges (calls, imports, inherits) that grep cannot find.

### Quick Reference

| Task | Command |
|------|---------|
| Root cause / "why does X happen?" | `graphify query "question"` |
| "How does A connect to B?" | `graphify path "A" "B"` |
| "What is X?" | `graphify explain "X"` |
| "What are the main hubs?" | `graphify god-nodes --top 10` |
| Update after code changes | `graphify update .` |
| Rebuild from scratch | `graphify extract . --code-only && graphify cluster-only .` |

### Git Hooks (Auto-Maintenance)
- `post-commit` — rebuilds graph on every commit (background, AST-only)
- `post-checkout` / `post-merge` — rebuilds on branch switch/pull
- After `git pull`: run `graphify update .` to sync

**No exceptions.** If you catch yourself reaching for grep/read — stop and run `graphify query` instead.
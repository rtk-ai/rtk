# DeepSeek Harness (DSH)

DSH loads instructions directly from `AGENTS.md`. It does not expand `@RTK.md`
imports, so RTK installs an inline block with DSH-specific markers.

```sh
rtk init --agent dsh                 # AGENTS.md in the current directory
rtk init -g --agent dsh              # $DSH_HOME/AGENTS.md (default ~/.dsh)
rtk init --agent dsh --dry-run       # preview without writing
rtk init --agent dsh --show          # show the selected instruction file
rtk init --agent dsh --uninstall     # remove only the DSH block
rtk init -g --agent dsh --uninstall  # remove the global block
```

Run project installation from the project root and start a new DSH session.
The `agent-instructions` plugin must be enabled with a positive instruction
budget and `AGENTS.md` among its project candidates (the default). For a custom
plugin `dshHome`, set `DSH_HOME` to that same path when running global init.
Relative `DSH_HOME` paths resolve from the current directory; `~`, `~/` and
`~\` prefixes expand to the user's home directory.

Installation is idempotent. Changes to an existing file save its previous content
to `AGENTS.md.bak`; subsequent changes replace that backup. Uninstall preserves
content outside the DSH markers, other RTK integrations, and the instruction file
itself (which can be empty afterward). Malformed or duplicate markers produce an
error without modifying the file. Hook, patch and filter flags are unsupported.

This is **prompt-level guidance**, not automatic command rewriting. DSH chooses
whether to follow the instructions. No hook or Cordis configuration is installed.
The DSH Claude Code compatibility bridge in the inspected source ignores
`updatedInput`, so the Claude rewrite hook is not used.

To verify, ask a new DSH session to inspect a repository with `rtk git status`.
Check the actual shell call and run `rtk gain` to inspect recorded savings.

DSH references: [instruction loading](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/context/agent-instructions)
and [home paths](https://github.com/deepseek-ai/deepseek-harness/tree/master/packages/util/home-paths).

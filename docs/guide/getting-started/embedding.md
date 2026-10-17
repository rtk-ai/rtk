---
title: Embedding rtk in your own agent harness
description: Call rtk rewrite from your own bash tool instead of installing a shipped hook — the exit-code contract, where the call goes, fail-open rules, and per-session accounting
sidebar:
  order: 5
---

# Embedding rtk in your own agent harness

This page is for people who build an agent harness and want the rewrite step inside their own bash tool, rather than installing one of the hooks listed in [Supported Agents](supported-agents.md). The Pi, OpenCode and Hermes plugins and the legacy shell hooks are thin delegates around one binary call, `rtk rewrite`; the Rust-binary hooks that `rtk init` installs today (`rtk hook claude` and friends) run the same registry in-process. For a host of your own, `rtk rewrite` is the whole integration surface. Everything below was verified against rtk 0.48.0.

## What `rtk rewrite` returns

`rtk rewrite "<command>"` prints the rewritten command to stdout (no trailing newline) and reports its decision in the exit code. The contract is defined in [`src/hooks/rewrite_cmd.rs`](https://github.com/rtk-ai/rtk/blob/master/src/hooks/rewrite_cmd.rs):

| Exit | Stdout | Meaning | What your host should do |
|------|--------|---------|--------------------------|
| 0 | rewritten command | Rewrite, and an explicit Claude Code allow rule matched | Run the rewritten command |
| 1 | empty | No RTK equivalent | Run the original command |
| 2 | empty | A Claude Code deny rule matched | Your call. The rule comes from Claude Code's settings, so the usual choice is to run the original command and let your own policy decide |
| 3 | rewritten command | Rewrite, with an "ask" rule matched or no rule matched at all | Run the rewritten command |

The exit code carries a permission verdict read from Claude Code's settings files (`.claude/settings.json` in the project and `~/.claude/settings.json`, plus their `.local` variants). Outside Claude Code those files usually do not exist, so no rule matches, the verdict is `Default`, and **exit 3 is the ordinary answer for every rewritable command**:

```bash
$ rtk rewrite "git status"; echo " -> exit $?"
rtk git status -> exit 3
$ rtk rewrite "htop"; echo " -> exit $?"
 -> exit 1
```

A host that treats 3 as anything other than 0 has RTK silently switched off. Treat 0 and 3 identically: when stdout is non-empty, that is the command to run. On a machine that also has Claude Code installed, its allow and deny rules show up as 0 and 2; treating 0 and 3 alike keeps your host indifferent to that.

Three more details of the call:

- **Read stdout only.** Stderr can carry advisory lines: a once-a-day "No hook installed" notice on machines where `~/.claude` exists without an RTK hook, or an `RTK_DISABLED=1 detected` message.
- **Compare output with input.** A command that already starts with `rtk` comes back unchanged (still exit 3), and there is nothing to substitute.
- **Check `RTK_DISABLED` yourself before spawning `rtk rewrite`**, as the Pi extension does. Releases up to 0.48.0 honor it only as an in-command prefix (`rtk rewrite "RTK_DISABLED=1 git status"` exits 1); [rtk-ai/rtk#3917](https://github.com/rtk-ai/rtk/pull/3917) makes `rtk rewrite` honor the exported variable as well.

`rtk rewrite` does not run the command and does not open the savings database. Compound commands are handled (`cargo fmt --all && cargo test` becomes `rtk cargo fmt --all && rtk cargo test`); commands containing a file redirect (`> out.txt`) or command substitution pass through with exit 1; a file-descriptor duplication such as `2>&1` does not block the rewrite.

## Where the call goes

Run the rewrite **after your own approval gate and before you spawn the shell**:

```
agent asks for "git status"
  -> your policy check or permission prompt sees "git status"
  -> rtk rewrite "git status"        prints "rtk git status", exit 3
  -> the shell runs "rtk git status"
  -> the model reads the filtered output
```

A person approving a command then sees the command the agent asked for, not the rewritten one, and allow or deny patterns written for `git ...` keep matching. If your transcript shows the executed command, show the rewritten form there so the substitution stays visible.

## Fail open

The command must always run, with or without rtk. Run the original command unchanged when:

- `rtk` is not on `PATH`
- `rtk --version` reports a version older than 0.25.0, the release that added `rtk rewrite` (the shipped hooks check for 0.23.0; on 0.23.0 and 0.24.0 the call exits 2 with nothing on stdout, one more reason to run the original on exit 2 rather than block)
- `rtk rewrite` has not answered within about 2 seconds (the Pi and Hermes hooks use a 2 s timeout; a rewrite normally answers in under 20 ms)
- it prints nothing to stdout
- it exits with any code other than 0 or 3, or crashes
- spawning it fails with `E2BIG` because the command is very long (`ARG_MAX` is 1 MiB on macOS)

Probe `rtk --version` once per host process and remember the answer; do not re-probe on every command. The reference implementation is the Pi extension, [`hooks/pi/rtk.ts`](https://github.com/rtk-ai/rtk/blob/master/hooks/pi/rtk.ts): about 140 lines covering the version probe, the timeout, and the 0/3 handling.

## A minimal sketch

```python
import shutil
import subprocess

TIMEOUT_S = 2


def rtk_ready() -> bool:
    """Probe once per process and remember the result."""
    if shutil.which("rtk") is None:
        return False
    try:
        out = subprocess.run(["rtk", "--version"], capture_output=True, text=True,
                             timeout=TIMEOUT_S).stdout          # "rtk 0.48.0"
        major, minor, _ = (int(x) for x in out.split()[1].split("."))
        return (major, minor) >= (0, 25)
    except (OSError, subprocess.TimeoutExpired, ValueError, IndexError):
        return False


def rewrite(command: str) -> str:
    """Return the command to spawn. Every failure returns the input unchanged."""
    try:
        r = subprocess.run(["rtk", "rewrite", command], capture_output=True, text=True,
                           timeout=TIMEOUT_S)
    except (OSError, subprocess.TimeoutExpired):    # missing, E2BIG, crash, too slow
        return command
    if r.returncode in (0, 3) and r.stdout.strip():
        return r.stdout.strip()
    return command                                   # 1: no filter, 2: deny, anything else
```

Call `rewrite()` on the approved command string and hand the result to the same shell you would have used anyway. `rtk` has to be on the `PATH` of that shell too.

## Accounting per invocation

Every filtered command records its raw and filtered sizes in a SQLite history that `rtk gain` reads. The default file is shared by every project and session run under the same account, so a host that runs several sessions at once cannot attribute savings from it. Give each command its own database instead: point `RTK_DB_PATH` at a fresh temporary path, read the totals once when the command finishes, then delete the file.

```bash
export RTK_DB_PATH=/tmp/session-42/cmd-7.db   # a path that does not exist yet
rtk git status                                # the filtered command creates the file
rtk gain --format json                        # totals for that file only
rm "$RTK_DB_PATH"
```

`rtk gain --format json` against a path that does not exist yet creates it and returns zeros. Without `--all` the document holds only a `summary` object, with these fields (this one is after a single `rtk git status` in a clean checkout):

```json
{
  "summary": {
    "total_commands": 1,
    "total_input": 18,
    "total_output": 13,
    "total_saved": 5,
    "avg_savings_pct": 27.77777777777778,
    "total_time_ms": 26,
    "avg_time_ms": 26
  }
}
```

`total_input`, `total_output`, and `total_saved` are estimated tokens (`bytes / 4`); `total_time_ms` and `avg_time_ms` are execution time in milliseconds. `RTK_DB_PATH` takes precedence over `tracking.database_path` in `config.toml`, and an empty pre-created file works as well as a missing one. The variable has no effect on `rtk rewrite`, which never opens the database.

## Sandbox environment

Set `RTK_TELEMETRY_DISABLED=1` in the environment of every rtk process. Telemetry is opt-in (it requires consent given through `rtk init` or `rtk telemetry enable`), so a fresh sandbox sends nothing anyway; the variable states the intent and holds if a shared config ever turns it on. See [Telemetry & Privacy](../resources/telemetry.md).

## Escape hatch and exclusions

- `rtk proxy <cmd>` runs a command with raw, unfiltered output. It is still recorded in the history, at zero savings. Use it when the agent needs the complete output of a command RTK would otherwise condense.
- Commands that must never be rewritten go in `[hooks] exclude_commands` in `config.toml`; `rtk rewrite` answers exit 1 for a matching command. See [Excluding commands from auto-rewrite](configuration.md#excluding-commands-from-auto-rewrite) for the matching rules.

```toml
[hooks]
exclude_commands = ["git rebase", "git cherry-pick"]
```

## What the numbers mean

The figures from `rtk gain` measure bash output bytes, converted to tokens by estimate. They are not a bill, and they do not translate into cost at the same rate. Read [How RTK Savings Work](../resources/savings-explained.md) before putting them on a dashboard.

## Verified against

rtk 0.48.0 on macOS. Exit codes come from `src/hooks/rewrite_cmd.rs` and `hooks/pi/rtk.ts`, the `rtk gain --format json` field names from `ExportSummary` in `src/analytics/gain.rs` (mirroring `GainSummary` in `src/core/tracking.rs`), and every command above was run against that binary.

# Pi Extension

RTK's Pi integration is a TypeScript extension installed to Pi's extension directory.

## Install

```bash
rtk init --agent pi
```

By default this writes:

```text
~/.pi/agent/extensions/rtk.ts
```

If `PI_CODING_AGENT_DIR` is set, RTK installs under that directory instead:

```text
$PI_CODING_AGENT_DIR/extensions/rtk.ts
```

Restart Pi or run `/reload` after installing.

## How it works

The extension listens for Pi `tool_call` events, targets only the built-in `bash` tool, and mutates `event.input.command` in place after delegating to:

```bash
rtk rewrite "<command>"
```

For example, Pi's raw `git status` tool call becomes `rtk git status` before execution. All command matching stays in the RTK binary.

## Failure behavior

The extension fails open. If `rtk` is unavailable, `rtk rewrite` times out, or any error occurs, the original command is left unchanged so Pi can continue normally.

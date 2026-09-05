# RTK agent integration examples

These examples document the process boundary for SDKs and external workers.
They are intentionally offline: they do not contain API keys, change host
permissions, or assume that a Windows executable is available inside WSL, a
container, or a remote worker.

## Direct SDK/worker route

When an SDK is about to execute a supported shell command, pass the typed RTK
argument vector to the local `rtk` executable in the same environment where the
command will run. Set the output audience and semantic token budget for that
invocation; do not reuse a global environment variable across workers. If the
route is unsupported, machine-readable, redirected, or otherwise exact, invoke
the original tool directly and preserve its exit code and bytes.

The TypeScript and Python examples show this adapter boundary. They are recipes,
not live SDK compatibility claims; SDK callback names and tool schemas must be
checked against the installed SDK before wiring them in.

## Host/process matrix

| Environment | Required setup | Status in this checkout |
|---|---|---|
| Windows Codex host | Install RTK and Codex config in the same Windows profile | local config inspected; live child probe unavailable |
| WSL/Linux worker | Install a Linux RTK binary and configure that worker's `CODEX_HOME` | unverified |
| Container/remote worker | Install RTK inside the image/host and configure its own instructions/MCP | unverified |
| Separate `CODEX_HOME` | Run `rtk doctor --agent codex --format json` with that environment | deterministic fake-home test |

Use `python scripts/verify-agent-coverage.py` for the offline fixture check.
Use `python scripts/verify-agent-coverage.py --live-command -- <typed command>`
only when an operator explicitly wants to run that command.

## Ruflo and other external executors

An external executor is a separate process boundary. Its lifecycle hooks do not
prove that its child command tool uses RTK. Configure the executor's own command
adapter, install RTK in the child environment, and verify that environment
directly. No Ruflo live path was available during this implementation, so this
repository makes no universal Ruflo interception claim.

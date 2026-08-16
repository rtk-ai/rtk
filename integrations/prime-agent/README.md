# Prime Agent integration

`rtk-rewrite.ts` is a Prime Agent extension that intercepts `%%bash` cells in
its `ipython` tool and rewrites supported standalone commands through
`rtk rewrite`. Unsupported commands and RTK failures pass through unchanged.
The extension preserves `%%bash` options and treats RTK exit status `3` with
rewritten output as a successful rewrite.

Because Prime Agent is built on Pi, the extension uses the same
`@earendil-works/pi-coding-agent` `ExtensionAPI`. At load time it probes for
`rtk >= 0.23.0` and disables itself with a warning if RTK is missing or too
old. Set `RTK_DISABLED=1` to bypass rewriting.

Install it using RTK's installer:

```bash
rtk init --agent prime-agent        # project-scoped (.prime/extensions/rtk.ts)
rtk init --agent prime-agent --global # user-scoped (~/.prime/agent/extensions/rtk.ts)
```

Or copy `integrations/prime-agent/rtk-rewrite.ts` manually into
`~/.prime/agent/extensions/` and restart Prime Agent. RTK can also be
bypassed per-session with `RTK_DISABLED=1`.

## Smoke test

With the extension installed, these cells should preserve the first command
and rewrite the second when `ls` is supported by the installed RTK version:

```python
%%bash -e
echo hello
```

```python
%%bash
ls -la /tmp
```

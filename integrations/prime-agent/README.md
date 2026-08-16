# Prime Agent integration

`rtk-rewrite.ts` is a Prime Agent extension that intercepts `%%bash` cells in
its `ipython` tool and rewrites supported standalone commands through
`rtk rewrite`. Unsupported commands and RTK failures pass through unchanged.
The extension preserves `%%bash` options and treats RTK exit status `3` with
rewritten output as a successful rewrite.

Install it by copying the extension into Prime Agent's global extension
folder (`~/.prime/agent/extensions/`) and restarting Prime Agent.

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

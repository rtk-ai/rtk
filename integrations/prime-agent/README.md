# Prime Agent integration

`rtk-rewrite.ts` is a Prime Agent extension that intercepts `%%bash` cells in
its `ipython` tool and rewrites supported standalone commands through
`rtk rewrite`. Unsupported commands and RTK failures pass through unchanged.

Install it by copying the extension into Prime Agent's global extension
folder (`~/.prime/agent/extensions/`) and restarting Prime Agent.

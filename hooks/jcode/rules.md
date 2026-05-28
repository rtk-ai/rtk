<!-- rtk-instructions jcode v1 -->
# RTK command usage for Jcode

When using Jcode's `bash` or `shell_exec` tool, prefer RTK-wrapped commands for supported developer workflows. RTK filters noisy command output before it reaches the model context.

Use RTK for common commands such as:

- `rtk git status` instead of `git status`
- `rtk git diff` instead of `git diff`
- `rtk git log` instead of `git log`
- `rtk cargo test` instead of `cargo test`
- `rtk cargo build` instead of `cargo build`
- `rtk npm install` instead of `npm install`
- `rtk pytest` instead of `pytest`
- `rtk grep <pattern> <path>` instead of raw `grep` when compact output is acceptable

Rules:

- If a command already starts with `rtk`, do not add another `rtk`.
- Use raw commands only when RTK is unavailable, unsuitable, or exact raw output is required.
- For chained commands, prefix each supported command separately when practical, for example `rtk cargo fmt && rtk cargo test`.
- RTK is safe for unsupported commands because it passes through when no filter applies.
<!-- /rtk-instructions -->

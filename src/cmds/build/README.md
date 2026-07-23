# Build Commands

Native wrappers for build systems whose output needs stateful filtering.

| File | Commands | Strategy |
|------|----------|----------|
| `bazel_cmd.rs` | `bazel`, `bazelisk` | Compact `test`/`lint`/`build`, stream `run`, pass `query` through raw |

## Bazel

Bazel uses flat argument parsing so startup options before the subcommand are
preserved exactly. `bazel query` is intentionally raw by default because target
label output is often consumed by scripts and pipelines.

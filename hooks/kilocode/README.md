# Kilo Code Plugin

> Part of [`hooks/`](../README.md)

## Specifics

- TypeScript plugin that intercepts `tool.execute.before` and mutates eligible Bash or shell commands in place.
- Delegates rewrite decisions to `rtk rewrite`; machine-readable output flags stay untouched.
- Locates RTK on Windows, Linux, and WSL, with `RTK_KILO_BIN` as an explicit override.
- Uses Kilo's `shell.env` hook to make the resolved executable directory reachable on the tool's `PATH` (appended only when missing, never reordering existing entries), without changing shell syntax or the host process environment.
- Installed to `~/.config/kilo/plugin/rtk.ts` by `rtk init -g --agent kilocode`.
- Honors Kilo's `XDG_CONFIG_HOME` and `KILO_CONFIG_DIR` configuration-directory overrides.
- Set `RTK_KILO_BASH_PLUGIN=0` to disable rewrites for a session.

## Install From Source

In Bash, from the root of a checkout of this repository, with Rust 1.91 or newer:

```bash
cargo install --path . --locked --root "$HOME/.local" --force
"$HOME/.local/bin/rtk" init -g --agent kilocode
```

This builds the current checkout and installs its embedded plugin. `--force`
replaces an existing executable at that installation root. The repository's
`install.sh` downloads a prebuilt official `rtk-ai/rtk` release instead of
building the current checkout, so use the source install to try plugin changes
that are not released yet. After updates, repeat both commands and restart Kilo
to reload the plugin.

An earlier plugin could find `~/.local/bin/rtk` by absolute path, then emit
`rtk git status` into a shell whose `PATH` did not contain that directory.
The environment hook makes the resolved executable directory available to
the tool, including all commands in chains and pipelines. Paths containing
spaces or shell metacharacters remain environment values, not shell code.
The directory is appended to the end of the tool's `PATH` only when it is not
already listed, so the order of existing entries (virtual environments,
version-manager shims, other tool directories) never changes and no other
executable in that directory gains precedence. An `rtk` that appears earlier on
the tool's `PATH` therefore still takes precedence over the resolved one.
Repeated hook calls do not duplicate the entry. PATH-based discovery also
resolves an absolute executable so changing the tool's working directory cannot
break it. Windows overrides use one canonical `PATH` key to avoid
case-insensitive duplicates. A missing binary or disabled plugin registers
neither hook. No global shell configuration is required by the plugin.

`RTK_KILO_BIN` must point to an executable named `rtk` (`rtk.exe` on Windows),
because the rewrite engine emits that name. A renamed executable or an install
directory containing the platform's PATH separator (`:` on POSIX, `;` on
Windows) disables the plugin rather than executing a different binary.

For general command-line use outside Kilo, configure `~/.local/bin` in the
user's login and desktop-session environment. An interactive-only `.bashrc`
export does not configure applications started from the desktop launcher.
On systemd-based desktops, see [environment.d](https://www.freedesktop.org/software/systemd/man/latest/environment.d.html).
This user environment setup is separate from the plugin's tool environment.

To remove only the installed Kilo plugin:

```bash
"$HOME/.local/bin/rtk" init -g --agent kilocode --uninstall
```

## Verification

From the repository root in Bash, with Rust 1.91 or newer and the `rustfmt`
and `clippy` components installed, plus Node.js 24 or newer for the plugin tests:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked
cargo test --test init_kilocode_test --locked
cargo test --test hook_detection_multi_agent_test --locked
cargo test --all --locked
node --test hooks/kilocode/rtk.test.mjs
```

The regression tests run the command-line interface in temporary directories:
global installation writes the embedded plugin, repeated installation is a
no-op, dry-run writes nothing, uninstall preserves other plugins, and
project-scoped install and uninstall fail with the global command to use.
`KILO_CONFIG_DIR` takes precedence over `XDG_CONFIG_HOME`; empty overrides are
ignored. With neither override, the plugin goes under `~/.config/kilo`.

The multi-agent detection tests verify that an installed Kilo plugin suppresses
missing-hook warnings in `rtk gain` and the daily check. An outdated Claude Code
hook still warns even when the Kilo plugin is installed; Claude-specific hook
diagnostics remain independent of Kilo installation.

The plugin tests exercise the actual hooks and execute a temporary mock binary
outside `PATH`, including chains, pipelines, quoted directory names, relative
overrides and PATH entries, empty or custom tool `PATH`, preserved `PATH` order
(no shadowing of earlier entries), opt-out, and missing binaries. Probes are
isolated from host installations. The mock binary is a POSIX shell script, so
every plugin test is skipped on Windows; Windows `PATH` key normalization is
only simulated, by overriding `process.platform` on POSIX, without invoking a
Windows shell.

The command dispatch lives in `src/lib.rs`; `src/main.rs` only calls
`rtk::entry()`. Install and uninstall helpers live in
`src/hooks/init/kilocode.rs`, not in the instruction-only agents module.

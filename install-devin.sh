#!/usr/bin/env bash
# One-command installer for RTK + Devin CLI integration.
# Run from the repo root: bash install-devin.sh
set -euo pipefail

cd "$(dirname "$0")"

RED='\033[31m'
GREEN='\033[32m'
RESET='\033[0m'

err() {
    echo -e "${RED}error:${RESET} $*" >&2
    exit 1
}

ok() {
    echo -e "${GREEN}ok:${RESET} $*"
}

# 1. Verify Rust toolchain
if ! command -v cargo &>/dev/null; then
    err "cargo not found. Install Rust first: https://rustup.rs"
fi

# 2. Build and install the rtk binary
ok "building and installing rtk (this may take a minute)..."
cargo install --path . --locked --force

# 3. Make sure rtk is reachable in this script's PATH
RTK_BIN="$(command -v rtk || true)"
if [[ -z "$RTK_BIN" ]]; then
    # cargo install defaults to ~/.cargo/bin; try adding it for this session
    export PATH="${HOME}/.cargo/bin:${PATH}"
    RTK_BIN="$(command -v rtk || true)"
fi
if [[ -z "$RTK_BIN" ]]; then
    err "rtk was installed but is not on PATH. Add ${HOME}/.cargo/bin to your PATH and re-run."
fi

ok "rtk installed at ${RTK_BIN}"

# 4. Install Devin CLI hooks globally
ok "installing Devin CLI hooks..."
rtk init -g --agent devin --auto-patch

ok "Devin CLI integration is installed."
echo "   Restart Devin CLI for the hooks to take effect."
echo "   Test it by running: git status"

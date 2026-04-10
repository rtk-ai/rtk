#!/usr/bin/env bash
# Install RTK globally (builds from source, installs system-wide, requires sudo).
# Warns before overwriting any existing binary (e.g. from brew or another package manager).

set -euo pipefail

INSTALL_DIR="${1:-/usr/local/bin}"
INSTALL_PATH="${INSTALL_DIR}/rtk"
BINARY_PATH="./target/release/rtk"

if ! command -v cargo &>/dev/null; then
    echo "error: cargo not found"
    echo "install Rust: https://rustup.rs"
    exit 1
fi

# Warn if an existing rtk binary is present (and note where it came from)
if command -v rtk &>/dev/null; then
    EXISTING_PATH="$(command -v rtk)"
    EXISTING_VERSION="$(rtk --version 2>/dev/null || echo "unknown version")"
    echo "warning: existing rtk found at $EXISTING_PATH ($EXISTING_VERSION)"
    if [ "$EXISTING_PATH" != "$INSTALL_PATH" ]; then
        echo "         this install will shadow it (install path: $INSTALL_PATH)"
    else
        echo "         it will be overwritten"
    fi
    echo ""
    read -r -p "continue? [y/N] " REPLY
    case "$REPLY" in
        [yY][eE][sS]|[yY]) ;;
        *) echo "aborted."; exit 0 ;;
    esac
    echo ""
fi

echo "installing to: $INSTALL_PATH"

if [ -f "$BINARY_PATH" ] && [ -z "$(find src/ Cargo.toml Cargo.lock -newer "$BINARY_PATH" -print -quit 2>/dev/null)" ]; then
    echo "binary is up to date, skipping build"
else
    echo "building rtk (release)..."
    cargo build --release
fi

if [ ! -w "$INSTALL_DIR" ]; then
    echo "sudo required to write to $INSTALL_DIR"
    sudo install -m 755 "$BINARY_PATH" "$INSTALL_PATH"
else
    install -m 755 "$BINARY_PATH" "$INSTALL_PATH"
fi

echo "installed: $INSTALL_PATH"
echo "version:   $("$INSTALL_PATH" --version)"

#!/usr/bin/env bash
# rtk source installer — builds and installs rtk from source.
#
# Unlike install.sh (which downloads prebuilt upstream binaries), this script
# compiles rtk locally. Use it for forks or local changes — e.g. the
# telemetry-disabled build — where the published release artifacts do NOT
# contain your modifications.
#
# Supported platforms:
#   - Linux (Debian/Ubuntu and derivatives — apt-based)
#   - macOS (Apple Silicon and Intel)
#
# Usage:
#   # From a checkout of the repo:
#   ./install-from-source.sh
#
#   # Standalone (clones the repo first):
#   curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/<branch>/install-from-source.sh | bash
#
# Options:
#   --dir <path>     Install directory (default: $RTK_INSTALL_DIR or ~/.local/bin)
#   --branch <name>  Branch to build when cloning (default: $RTK_BRANCH or current)
#   --no-deps        Skip OS dependency installation (assume toolchain present)
#   --init           Run `rtk init` after install to wire up Claude Code hooks
#   --help           Show this help
#
# Environment:
#   RTK_INSTALL_DIR  Override install directory
#   RTK_REPO_URL     Git URL to clone when run standalone (default: upstream)
#   RTK_BRANCH       Branch to build/clone

set -euo pipefail

REPO_URL="${RTK_REPO_URL:-https://github.com/rtk-ai/rtk.git}"
BINARY_NAME="rtk"
INSTALL_DIR="${RTK_INSTALL_DIR:-$HOME/.local/bin}"
BRANCH="${RTK_BRANCH:-}"
INSTALL_DEPS=1
RUN_INIT=0

# --- Output helpers -------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { printf "${GREEN}[INFO]${NC} %s\n" "$1"; }
warn()  { printf "${YELLOW}[WARN]${NC} %s\n" "$1"; }
step()  { printf "${BLUE}[==>]${NC} %s\n" "$1"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$1" >&2; exit 1; }

usage() {
    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

# --- Argument parsing -----------------------------------------------------
while [ $# -gt 0 ]; do
    case "$1" in
        --dir)     INSTALL_DIR="${2:?--dir requires a path}"; shift 2;;
        --branch)  BRANCH="${2:?--branch requires a name}"; shift 2;;
        --no-deps) INSTALL_DEPS=0; shift;;
        --init)    RUN_INIT=1; shift;;
        --help|-h) usage;;
        *)         error "Unknown option: $1 (use --help)";;
    esac
done

# --- Privilege escalation helper -----------------------------------------
SUDO=""
need_sudo() {
    if [ "$(id -u)" -ne 0 ]; then
        if command -v sudo >/dev/null 2>&1; then
            SUDO="sudo"
        else
            error "Root privileges required to install system packages, but 'sudo' is not available. Re-run as root or use --no-deps."
        fi
    fi
}

# --- Platform detection ---------------------------------------------------
detect_platform() {
    case "$(uname -s)" in
        Linux*)
            if [ -f /etc/debian_version ] || command -v apt-get >/dev/null 2>&1; then
                PLATFORM="debian"
            else
                error "Unsupported Linux distribution. This script supports Debian/Ubuntu (apt). Install build-essential, pkg-config and Rust manually, then re-run with --no-deps."
            fi
            ;;
        Darwin*)
            PLATFORM="macos"
            ;;
        *)
            error "Unsupported operating system: $(uname -s)"
            ;;
    esac
    info "Platform: $PLATFORM ($(uname -m))"
}

# --- Dependency installation ---------------------------------------------
# rusqlite is built with the `bundled` feature, which compiles SQLite from C —
# a working C toolchain (cc) is mandatory in addition to the Rust toolchain.
install_deps_debian() {
    step "Installing build dependencies (apt)..."
    need_sudo
    $SUDO apt-get update -qq
    $SUDO apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates git
}

install_deps_macos() {
    step "Checking build dependencies (macOS)..."
    if ! xcode-select -p >/dev/null 2>&1; then
        info "Installing Xcode Command Line Tools (provides the C compiler)..."
        xcode-select --install || true
        error "Command Line Tools installation was triggered. Complete the GUI prompt, then re-run this script."
    fi
    info "Xcode Command Line Tools present."
}

ensure_rust() {
    if command -v cargo >/dev/null 2>&1; then
        info "Rust toolchain found: $(cargo --version)"
        return
    fi
    step "Rust toolchain not found — installing via rustup..."
    curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
    command -v cargo >/dev/null 2>&1 || error "Rust installation failed; install manually from https://rustup.rs"
    info "Installed: $(cargo --version)"
}

# --- Source resolution ----------------------------------------------------
# Determine the directory to build in. If the script lives inside an rtk
# checkout, build that. Otherwise clone the repo into a temp dir.
resolve_source() {
    SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
    if [ -f "$SCRIPT_DIR/Cargo.toml" ] && grep -q '^name = "rtk"' "$SCRIPT_DIR/Cargo.toml" 2>/dev/null; then
        SOURCE_DIR="$SCRIPT_DIR"
        info "Building from local checkout: $SOURCE_DIR"
        if [ -n "$BRANCH" ]; then
            warn "--branch ignored: building the checked-out tree as-is."
        fi
    else
        command -v git >/dev/null 2>&1 || error "git is required to clone the repository"
        SOURCE_DIR=$(mktemp -d)
        CLONED=1
        step "Cloning $REPO_URL..."
        if [ -n "$BRANCH" ]; then
            git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$SOURCE_DIR"
        else
            git clone --depth 1 "$REPO_URL" "$SOURCE_DIR"
        fi
    fi
}

# --- Build & install ------------------------------------------------------
build_and_install() {
    step "Building $BINARY_NAME (release)..."
    ( cd "$SOURCE_DIR" && cargo build --release )

    local binary="$SOURCE_DIR/target/release/$BINARY_NAME"
    [ -f "$binary" ] || error "Build succeeded but binary not found at $binary"

    step "Installing to $INSTALL_DIR..."
    mkdir -p "$INSTALL_DIR"
    install -m 755 "$binary" "$INSTALL_DIR/$BINARY_NAME"
    info "Installed: $INSTALL_DIR/$BINARY_NAME"
}

# --- Post-install ---------------------------------------------------------
verify_and_report() {
    local installed="$INSTALL_DIR/$BINARY_NAME"
    info "Version: $("$installed" --version)"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            warn "$INSTALL_DIR is not in your PATH. Add this to your shell profile:"
            warn "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac

    if [ "$RUN_INIT" -eq 1 ]; then
        step "Running '$BINARY_NAME init' to set up Claude Code hooks..."
        "$installed" init || warn "'$BINARY_NAME init' failed — run it manually later."
    else
        info "To enable Claude Code integration, run: $BINARY_NAME init"
    fi
}

cleanup() {
    if [ "${CLONED:-0}" -eq 1 ] && [ -n "${SOURCE_DIR:-}" ]; then
        rm -rf "$SOURCE_DIR"
    fi
}

main() {
    trap cleanup EXIT
    CLONED=0

    info "Installing $BINARY_NAME from source..."
    detect_platform

    if [ "$INSTALL_DEPS" -eq 1 ]; then
        case "$PLATFORM" in
            debian) install_deps_debian;;
            macos)  install_deps_macos;;
        esac
        ensure_rust
    else
        info "Skipping dependency installation (--no-deps)."
        command -v cargo >/dev/null 2>&1 || error "cargo not found and --no-deps set. Install Rust from https://rustup.rs"
    fi

    resolve_source
    build_and_install
    verify_and_report

    echo ""
    info "Done. Run '$BINARY_NAME --help' to get started."
}

main

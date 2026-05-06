#!/usr/bin/env sh
# rtk installer (Homeserve mirror) - https://github.com/HomeserveFR/rtk
# Usage: curl -fsSL https://raw.githubusercontent.com/HomeserveFR/rtk/refs/heads/homeserve/main/install.sh | sh

set -e

REPO="HomeserveFR/rtk"
BINARY_NAME="rtk"
INSTALL_DIR="${RTK_INSTALL_DIR:-$HOME/.local/bin}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() {
    printf "${GREEN}[INFO]${NC} %s\n" "$1"
}

warn() {
    printf "${YELLOW}[WARN]${NC} %s\n" "$1"
}

error() {
    printf "${RED}[ERROR]${NC} %s\n" "$1"
    exit 1
}

detect_os() {
    case "$(uname -s)" in
        Darwin)                OS="darwin";  ARCHIVE_EXT="tar.gz"; BINARY_FILE="rtk";;
        Linux)                 OS="linux";   ARCHIVE_EXT="tar.gz"; BINARY_FILE="rtk";;
        MINGW*|MSYS*|CYGWIN*)  OS="windows"; ARCHIVE_EXT="zip";    BINARY_FILE="rtk.exe";;
        *)                     error "Unsupported operating system: $(uname -s)";;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  ARCH="x86_64";;
        arm64|aarch64) ARCH="aarch64";;
        *)             error "Unsupported architecture: $(uname -m)";;
    esac
}

# Get latest release version
# Primary: parse the 302 redirect on /releases/latest (no API call, no rate limit).
# Fallback: the GitHub REST API (subject to 60 req/hour anonymous limit).
get_latest_version() {
    # Try the web redirect first — does not count against the API rate limit.
    VERSION=$(curl -sI "https://github.com/${REPO}/releases/latest" \
        | grep -i '^location:' \
        | sed -E 's|.*/tag/([^[:space:]]+).*|\1|' \
        | tr -d '\r')

    # Fallback to the REST API if the redirect didn't yield a tag.
    if [ -z "$VERSION" ]; then
        warn "Redirect lookup failed, falling back to GitHub API..."
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name":' \
            | sed -E 's/.*"([^"]+)".*/\1/')
    fi

    if [ -z "$VERSION" ]; then
        error "Failed to get latest version (GitHub API may be rate-limited; set RTK_VERSION=vX.Y.Z to pin)"
    fi
}

get_target() {
    case "$OS" in
        darwin)
            TARGET="${ARCH}-apple-darwin"
            ;;
        linux)
            if [ "$ARCH" != "x86_64" ]; then
                error "This Homeserve mirror only ships Linux binaries for x86_64. Detected: $ARCH"
            fi
            TARGET="x86_64-unknown-linux-musl"
            ;;
        windows)
            if [ "$ARCH" != "x86_64" ]; then
                error "This Homeserve mirror only ships Windows binaries for x86_64. Detected: $ARCH"
            fi
            TARGET="x86_64-pc-windows-msvc"
            ;;
    esac
}

extract_archive() {
    case "$ARCHIVE_EXT" in
        tar.gz)
            tar -xzf "$ARCHIVE" -C "$TEMP_DIR"
            ;;
        zip)
            if command -v unzip >/dev/null 2>&1; then
                unzip -q -o "$ARCHIVE" -d "$TEMP_DIR"
            elif command -v tar >/dev/null 2>&1 && tar --help 2>&1 | grep -q -- '--format'; then
                tar -xf "$ARCHIVE" -C "$TEMP_DIR"
            else
                error "Cannot extract zip: neither 'unzip' nor a zip-capable 'tar' is available."
            fi
            ;;
    esac
}

install() {
    info "Detected: $OS $ARCH"
    info "Target: $TARGET"
    info "Version: $VERSION"

    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY_NAME}-${TARGET}.${ARCHIVE_EXT}"
    TEMP_DIR=$(mktemp -d)
    ARCHIVE="${TEMP_DIR}/${BINARY_NAME}.${ARCHIVE_EXT}"

    info "Downloading from: $DOWNLOAD_URL"
    if ! curl -fsSL "$DOWNLOAD_URL" -o "$ARCHIVE"; then
        error "Failed to download binary"
    fi

    info "Extracting..."
    extract_archive

    mkdir -p "$INSTALL_DIR"
    mv "${TEMP_DIR}/${BINARY_FILE}" "${INSTALL_DIR}/${BINARY_FILE}"

    chmod +x "${INSTALL_DIR}/${BINARY_FILE}" 2>/dev/null || true

    rm -rf "$TEMP_DIR"

    info "Successfully installed ${BINARY_FILE} to ${INSTALL_DIR}/${BINARY_FILE}"
}

verify() {
    INSTALLED_PATH="${INSTALL_DIR}/${BINARY_FILE}"
    if "$INSTALLED_PATH" --version >/dev/null 2>&1; then
        info "Verification: $("$INSTALLED_PATH" --version)"
    elif command -v "$BINARY_NAME" >/dev/null 2>&1; then
        info "Verification: $($BINARY_NAME --version)"
    else
        warn "Binary installed at ${INSTALLED_PATH} but not on PATH yet."
        case "$OS" in
            windows)
                warn "On Windows / Git Bash, add to PATH (one-time):"
                warn "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc"
                warn "Then restart your terminal. Or add ${INSTALL_DIR} to the Windows User PATH via System Properties."
                ;;
            *)
                warn "Add to your shell profile:"
                warn "  export PATH=\"\$HOME/.local/bin:\$PATH\""
                ;;
        esac
    fi
}

main() {
    info "Installing $BINARY_NAME (Homeserve mirror)..."

    detect_os
    detect_arch
    get_target
    if [ -n "$RTK_VERSION" ]; then
        VERSION="$RTK_VERSION"
        info "Using pinned version from RTK_VERSION: $VERSION"
    else
        get_latest_version
    fi
    install
    verify

    echo ""
    info "Installation complete! Run '$BINARY_NAME --help' to get started."
}

main

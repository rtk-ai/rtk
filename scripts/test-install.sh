#!/usr/bin/env sh
# Tests for install.sh.
#
# Section 1 — General install-path checks:
#   OS/arch/target resolution (detect_os, get_target) across linux/darwin/windows,
#   and archive extraction mechanics for both tar.gz and zip (Windows uses zip/.exe).
#
# Section 2 — CWE-22 path traversal checks (issue #1250):
#   Archive-content verification rejects absolute paths and ".." components,
#   for both the tar.gz (default) and zip (Windows) archive-listing paths.

set -eu

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
INSTALL_SH="$REPO_ROOT/install.sh"

if [ ! -f "$INSTALL_SH" ]; then
    echo "FAIL: install.sh not found at $INSTALL_SH"
    exit 1
fi

# Referenced by the error() function in the real `install.sh` script
RED='' NC=''

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

FAIL=0
pass() { printf '  PASS: %s\n' "$1"; }
fail() { printf '  FAIL: %s\n' "$1"; FAIL=1; }
skip() { printf '  SKIP: %s\n' "$1"; }

# Extract a function body verbatim from the real install.sh so tests exercise
# the actual current source rather than a hand-copied replica that can drift.
extract_fn() {
    sed -n "/^$1() {/,/^}/p" "$INSTALL_SH"
}

ERROR_FN=$(extract_fn error)
DETECT_OS_FN=$(extract_fn detect_os)
GET_TARGET_FN=$(extract_fn get_target)
UNZIP_GUARD_LINE=$(grep -F 'command -v unzip' "$INSTALL_SH")

# Prefer python3, but some environments (e.g. python.org's Windows installer) only
# expose "python", so fall back to it once python3 installation is confirmed.
PY3=""
for cand in python3 python; do
    if command -v "$cand" >/dev/null 2>&1 && "$cand" -c 'import sys; sys.exit(0 if sys.version_info[0] >= 3 else 1)' >/dev/null 2>&1; then
        PY3="$cand"
        break
    fi
done

echo "==> Section 1: General install-path checks"

# --- detect_os() ---

run_detect_os() {
    (
        eval "$ERROR_FN"
        eval "$DETECT_OS_FN"
        MOCK_S="$1"
        uname() { case "$1" in -s) printf '%s' "$MOCK_S" ;; esac; }
        detect_os
        echo "OS=$OS"
    ) 2>&1
}

assert_os() {
    label="$1" mock="$2" expected="$3"
    if out=$(run_detect_os "$mock"); then rc=0; else rc=$?; fi
    if [ "$rc" -eq 0 ] && [ "$out" = "OS=$expected" ]; then
        pass "$label"
    else
        fail "$label (rc=$rc out='$out')"
    fi
}

assert_os_error() {
    label="$1" mock="$2"
    if out=$(run_detect_os "$mock"); then rc=0; else rc=$?; fi
    if [ "$rc" -ne 0 ]; then
        pass "$label"
    else
        fail "$label (expected error, got rc=0 out='$out')"
    fi
}

assert_os "detect_os: MINGW64 (Git Bash) -> windows" "MINGW64_NT-10.0-22631" "windows"
assert_os "detect_os: MSYS -> windows" "MSYS_NT-10.0" "windows"
assert_os "detect_os: CYGWIN -> windows" "CYGWIN_NT-10.0" "windows"
assert_os "detect_os: Linux -> linux (regression)" "Linux" "linux"
assert_os "detect_os: Darwin -> darwin (regression)" "Darwin" "darwin"
assert_os_error "detect_os: unrecognized uname -s still errors" "FreeBSD"

# --- get_target() ---

run_get_target() {
    os="$1" arch="$2"
    (
        eval "$ERROR_FN"
        eval "$GET_TARGET_FN"
        OS="$os"
        ARCH="$arch"
        BINARY_NAME="rtk"
        get_target
        echo "TARGET=$TARGET ARCHIVE_EXT=$ARCHIVE_EXT BINARY_FILE=$BINARY_FILE"
    ) 2>&1
}

assert_target() {
    label="$1" os="$2" arch="$3" expected="$4"
    if out=$(run_get_target "$os" "$arch"); then rc=0; else rc=$?; fi
    if [ "$rc" -eq 0 ] && [ "$out" = "$expected" ]; then
        pass "$label"
    else
        fail "$label (rc=$rc out='$out')"
    fi
}

assert_target_error() {
    label="$1" os="$2" arch="$3"
    if out=$(run_get_target "$os" "$arch"); then rc=0; else rc=$?; fi
    if [ "$rc" -ne 0 ]; then
        pass "$label"
    else
        fail "$label (expected error, got rc=0 out='$out')"
    fi
}

assert_target "get_target: windows/x86_64 -> msvc target + zip/.exe" \
    "windows" "x86_64" "TARGET=x86_64-pc-windows-msvc ARCHIVE_EXT=zip BINARY_FILE=rtk.exe"
assert_target_error "get_target: windows/aarch64 is unsupported" "windows" "aarch64"
assert_target "get_target: linux/x86_64 -> tar.gz/no-ext (regression)" \
    "linux" "x86_64" "TARGET=x86_64-unknown-linux-musl ARCHIVE_EXT=tar.gz BINARY_FILE=rtk"
assert_target "get_target: darwin/x86_64 -> tar.gz/no-ext (regression)" \
    "darwin" "x86_64" "TARGET=x86_64-apple-darwin ARCHIVE_EXT=tar.gz BINARY_FILE=rtk"

# --- missing-unzip dependency guard ---

mkdir -p "$TMPDIR/empty_bin"

run_missing_unzip_guard() {
    (
        PATH="$TMPDIR/empty_bin"
        eval "$ERROR_FN"
        eval "$UNZIP_GUARD_LINE"
        echo "GUARD_DID_NOT_FIRE"
    ) 2>&1
}

if out=$(run_missing_unzip_guard); then rc=0; else rc=$?; fi
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -qF "unzip is required"; then
    pass "missing-unzip guard fires when unzip absent from PATH"
else
    fail "missing-unzip guard did not fire as expected (rc=$rc out='$out')"
fi

# --- extraction mechanics (real tar/unzip, no network) ---

extract_archive() {
    case "$1" in
        *.zip) unzip -q "$1" -d "$2" ;;
        *) tar -xzf "$1" -C "$2" ;;
    esac
}

if command -v unzip >/dev/null 2>&1 && [ -n "$PY3" ]; then
    "$PY3" - "$TMPDIR" <<'PY'
import sys, zipfile
base = sys.argv[1]
with zipfile.ZipFile(f"{base}/exe.zip", "w") as z:
    z.writestr("rtk.exe", "dummy windows binary content")
PY
    EXTRACT_DIR="$TMPDIR/extracted"
    DEST_DIR="$TMPDIR/dest"
    mkdir -p "$EXTRACT_DIR" "$DEST_DIR"
    extract_archive "$TMPDIR/exe.zip" "$EXTRACT_DIR"
    if [ -f "$EXTRACT_DIR/rtk.exe" ]; then
        mv "$EXTRACT_DIR/rtk.exe" "$DEST_DIR/rtk.exe"
        chmod +x "$DEST_DIR/rtk.exe"
        if [ -x "$DEST_DIR/rtk.exe" ]; then
            pass "zip extraction places and chmods rtk.exe correctly"
        else
            fail "rtk.exe not executable after chmod"
        fi
    else
        fail "rtk.exe missing after zip extraction"
    fi
else
    skip "zip extraction mechanics test (requires unzip and a python3-compatible interpreter)"
fi

echo ""
echo "==> Section 2: CWE-22 path traversal checks (issue #1250)"

if [ -z "$PY3" ]; then
    echo "SKIP: no python3-compatible interpreter found — crafted archive tests require one"
else
    # The check replicated from install.sh (keep in sync with install.sh).
    # Returns 0 when archive is safe, 1 when unsafe.
    check_archive() {
        case "$1" in
            *.zip) list_cmd="unzip -Z1" ;;
            *)     list_cmd="tar -tzf" ;;
        esac
        if $list_cmd "$1" 2>/dev/null | grep -qE '^/|(^|/)\.\.(/|$)'; then
            return 1
        fi
        return 0
    }

    # --- Build safe archive using standard tar ---
    mkdir -p "$TMPDIR/safe_src"
    printf '#!/bin/sh\necho rtk\n' > "$TMPDIR/safe_src/rtk"
    (cd "$TMPDIR/safe_src" && tar -czf "$TMPDIR/safe.tgz" rtk)

    # --- Build crafted malicious tar archives with python ---
    "$PY3" - "$TMPDIR" <<'PY'
import sys, tarfile, io

base = sys.argv[1]


def make(name, entry):
    with tarfile.open(f"{base}/{name}", "w:gz") as t:
        info = tarfile.TarInfo(name=entry)
        data = b"pwned"
        info.size = len(data)
        t.addfile(info, io.BytesIO(data))


make("traversal.tgz", "../etc/evil")
make("absolute.tgz", "/tmp/evil_abs")
make("middle.tgz", "rtk/../../../etc/evil")
make("end_dotdot.tgz", "rtk/..")
PY

    echo "==> Functional checks (tar.gz)"

    if check_archive "$TMPDIR/safe.tgz"; then
        pass "safe tar archive accepted"
    else
        fail "safe tar archive rejected (false positive)"
    fi

    for bad in traversal absolute middle end_dotdot; do
        if check_archive "$TMPDIR/$bad.tgz"; then
            fail "$bad tar archive accepted (should be rejected)"
        else
            pass "$bad tar archive rejected"
        fi
    done

    if command -v unzip >/dev/null 2>&1; then
        # --- Build safe + crafted malicious zip archives with python ---
        "$PY3" - "$TMPDIR" <<'PY'
import sys, zipfile

base = sys.argv[1]


def make(name, entry):
    with zipfile.ZipFile(f"{base}/{name}", "w") as z:
        z.writestr(entry, "pwned")


make("safe.zip", "rtk.exe")
make("traversal.zip", "../etc/evil")
make("absolute.zip", "/tmp/evil_abs")
make("middle.zip", "rtk/../../../etc/evil")
make("end_dotdot.zip", "rtk/..")
PY

        echo "==> Functional checks (zip)"

        if check_archive "$TMPDIR/safe.zip"; then
            pass "safe zip archive accepted"
        else
            fail "safe zip archive rejected (false positive)"
        fi

        for bad in traversal absolute middle end_dotdot; do
            if check_archive "$TMPDIR/$bad.zip"; then
                fail "$bad zip archive accepted (should be rejected)"
            else
                pass "$bad zip archive rejected"
            fi
        done
    else
        skip "zip archive traversal checks (unzip not available)"
    fi

    echo "==> Regression guard"

    if grep -qF 'tar -tzf' "$INSTALL_SH" && grep -qF '\.\.' "$INSTALL_SH"; then
        pass "install.sh still contains the tar.gz path-traversal check"
    else
        fail "install.sh is missing the tar.gz path-traversal check — was it removed?"
    fi

    if grep -qF 'unzip -Z1' "$INSTALL_SH"; then
        pass "install.sh still contains the zip path-traversal check"
    else
        fail "install.sh is missing the zip path-traversal check — was it removed?"
    fi
fi

echo ""
echo "==> Regression guard: Windows support markers"

check_marker() {
    if grep -qF "$1" "$INSTALL_SH"; then
        pass "install.sh still contains: $1"
    else
        fail "install.sh is missing: $1 — was Windows support removed?"
    fi
}

check_marker 'MINGW*|MSYS*|CYGWIN*'
check_marker 'x86_64-pc-windows-msvc'
check_marker 'command -v unzip'

echo ""
if [ "$FAIL" -eq 0 ]; then
    echo "All install.sh tests passed"
    exit 0
else
    echo "Some tests failed"
    exit 1
fi

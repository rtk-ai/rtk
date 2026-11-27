#!/usr/bin/env bash
set -uo pipefail

# check-embed-scope.sh — guard the path gating against include_str! drift.
#
# Every file rustc embeds is a build input, so .github/inert-paths.conf must
# never exclude one -- a change to an excluded file alone skips the build.
# hooks/*.md are the live example: they look like docs and are compiled in.
#
# Exit status: 0 clean, 1 drift.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=scope-lib.sh
. "$SCRIPT_DIR/scope-lib.sh"

# Runs a copy of this script in a fixture repo, where it scans only the fixture.
self_test() {
    local failed=0

    enter_fixture_repo || exit 1
    mkdir -p .github/scripts
    cp "$SCRIPT_DIR/check-embed-scope.sh" "$SCRIPT_DIR/scope-lib.sh" .github/scripts/

    # crate/ has its own Cargo.toml and the root has no fixture.txt, so a manifest
    # path resolved against anything but the crate comes out MISSING.
    mkdir -p crate/src crate/docs docs
    printf ':(exclude,top)docs/\n' > .github/inert-paths.conf
    touch crate/Cargo.toml
    echo x > crate/fixture.txt
    echo x > docs/x.md
    # Holds the excluded path as a substring, so only a whole-line match rejects it.
    echo x > crate/docs/x.md

    assert() {   # [probe=<path>] assert PASS|<first word of the report> <rust source>
        local want="$1" out rc path="${probe:-crate/src/probe.rs}"
        printf '%s\n' "$2" > "$path"
        git add -Af
        out="$(bash .github/scripts/check-embed-scope.sh 2>&1)"
        rc=$?
        git rm -qf --cached "$path"
        rm -f "$path"
        if [ "$want" = PASS ]; then
            [ "$rc" -eq 0 ] && return
        elif [ "$rc" -eq 1 ] && grep -q "^$want" <<< "$out"; then
            return
        fi
        echo "FAIL: want $want, got rc=$rc, for: $2"
        printf '%s\n' "$out"
        failed=1
    }

    local manifest='env!("CARGO_MANIFEST_DIR")'
    assert PASS 'include_str!("../fixture.txt");'
    assert PASS 'include_bytes!("../fixture.txt");'
    # rustfmt wraps long paths onto their own line.
    assert PASS "$(printf 'include_str!(\n    "../fixture.txt"\n);')"
    assert PASS "include_str!(concat!($manifest, \"/fixture.txt\"));"
    assert PASS "$(printf 'include_str!(concat!(\n    %s,\n    "/fixture.txt"\n));' "$manifest")"
    assert PASS "include_str!(\"../fixture.txt\"); include_str!(concat!(env!(\"OUT_DIR\"), \"/gen.rs\"));"

    assert DRIFT 'include_str!("../../docs/x.md");'
    assert DRIFT "include_str!(concat!($manifest, \"/../docs/x.md\"));"
    # git quotes this path by default, and a file it cannot open would scan as empty.
    probe='crate/src/é.rs' assert DRIFT 'include_str!("../../docs/x.md");'

    assert MISSING 'include_str!("../nope.txt");'
    assert MISSING "include_str!(concat!($manifest, \"/nope.txt\"));"

    assert UNRECOGNISED 'include_str!(concat!("../", "fixture.txt"));'
    assert UNRECOGNISED "include_str!(concat!($manifest, \"/fix\", \"ture.txt\"));"
    assert UNRECOGNISED 'include_bytes!(concat!("../", "fixture.txt"));'
    # An embed right after a literal one must not fall into the literal's window.
    assert UNRECOGNISED 'include_str!("../fixture.txt"); include_str!(concat!("../", "fixture.txt"));'

    # The fixture root has no Cargo.toml, so this one has no crate to resolve against.
    probe=root.rs assert UNRESOLVED "include_str!(concat!($manifest, \"/fixture.txt\"));"

    assert "no embedded" 'fn main() {}'

    cd / || true
    rm -rf "$FIXTURE_REPO"

    if [ "$failed" -ne 0 ]; then
        echo "FAIL: check-embed-scope self-test"
        exit 1
    fi
    echo "PASS: check-embed-scope self-test"
}

if [ "${1:-}" = "--self-test" ]; then
    self_test
    exit 0
fi

cd "$SCRIPT_DIR/../.." || exit 1
CONF=.github/inert-paths.conf

load_pathspecs "$CONF" || { echo "$CONF is unreadable or lists no pathspecs"; exit 1; }

# `git ls-files -- <literal> :(exclude)...` returns nothing whether or not the
# exclude matches, so membership in the surviving set is the only usable test.
kept="$(git -c core.quotePath=false ls-files --cached --others --exclude-standard -- ':/' "${PATHSPECS[@]}")"

# GNU realpath's -m/--relative-to do not exist on macOS.
normalize_path() {
    local path="$1" seg
    local -a segs out=()
    IFS='/' read -ra segs <<< "$path"
    for seg in "${segs[@]}"; do
        case "$seg" in
            ''|.) ;;
            ..) [ "${#out[@]}" -gt 0 ] && out=("${out[@]:0:${#out[@]}-1}") ;;
            *) out+=("$seg") ;;
        esac
    done
    (IFS=/; echo "${out[*]}")
}

# The crate root env!("CARGO_MANIFEST_DIR") names for a source file.
manifest_dir() {
    local dir
    dir="$(dirname "$1")"
    while [ ! -e "$dir/Cargo.toml" ]; do
        [ "$dir" = . ] && return 1
        dir="$(dirname "$dir")"
    done
    echo "$dir"
}

EMBED_OPEN='include_(str|bytes)! *\('

# include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/x")), the one non-literal
# form this resolves. Matched on whole files flattened to one line.
MANIFEST_EMBED="$EMBED_OPEN"' *concat! *\( *env! *\( *"CARGO_MANIFEST_DIR" *\) *, *"[^"]+" *,? *\)'

# The tracked .rs files that can hold an embed; every form scanned below needs the
# macro name and its `!` adjacent, so that token always sits on one physical line.
# A git failure lists nothing, which the zero check at the end turns into a red run.
rust_sources() {
    git -c core.quotePath=false grep -lF -e 'include_str!' -e 'include_bytes!' -- '*.rs'
}

# Puts each macro on its own line. grep -o does not match overlapping windows, so
# without this the 80 characters trailing a literal embed swallow whichever embed
# follows it, and that one is never reported.
split_embeds() {
    awk '{gsub(/'"$EMBED_OPEN"'/, "\n&"); print}'
}

drift=0
checked=0

# rustfmt wraps long paths onto their own line, out of reach of a
# line-oriented grep.
while IFS= read -r hit; do
    src="${hit%%|*}"
    target="${hit#*|}"

    checked=$((checked + 1))
    case "$target" in
        CARGO_MANIFEST_DIR:*)
            base="$(manifest_dir "$src")"
            target="${target#CARGO_MANIFEST_DIR:}"
            resolved="${base:+$(normalize_path "$base/$target")}"
            ;;
        *) resolved="$(normalize_path "$(dirname "$src")/$target")" ;;
    esac
    if [ -z "$resolved" ]; then
        echo "UNRESOLVED: $src embeds $target"
        drift=1
        continue
    fi

    # rustc cannot embed a file that is not there. This is also how a rename
    # out of an embedded path would slip past the other checks.
    if [ ! -e "$resolved" ]; then
        echo "MISSING: $src embeds $target, resolving to $resolved"
        drift=1
        continue
    fi

    # A here-string, not a pipe: grep -q exits on the first match, and under
    # pipefail the writer's SIGPIPE would then report a kept file as excluded.
    if ! grep -Fxq -- "$resolved" <<< "$kept"; then
        echo "DRIFT: $src embeds $resolved, which $CONF excludes."
        echo "       A change to it alone would skip the build and ship stale bytes."
        drift=1
    fi
done < <(
    rust_sources | while IFS= read -r f; do
        esc="$(printf '%s' "$f" | sed 's/[&/\]/\\&/g')"
        tr '\n' ' ' < "$f" \
            | split_embeds \
            | grep -oE "$EMBED_OPEN"' *"[^"]+"|'"$MANIFEST_EMBED" \
            | sed -E -e '/CARGO_MANIFEST_DIR/s/.*"([^"]+)" *,? *\)$/'"$esc"'|CARGO_MANIFEST_DIR:\1/' \
                -e '/CARGO_MANIFEST_DIR/!s/.*"([^"]+)"$/'"$esc"'|\1/'
    done | sort -u
)

# A literal that does not follow the macro directly cannot be resolved here, so
# it has to be named rather than passed over -- otherwise an embed added as
# include_str!(concat!("../../docs/", "x.md")) would be invisible to this
# guard, which is the one thing standing between the inert list and a
# stale binary. The CARGO_MANIFEST_DIR form resolved above is removed first.
UNRECOGNISED="$(
    rust_sources | while IFS= read -r f; do
        tr '\n' ' ' < "$f" \
            | sed -E "s/$MANIFEST_EMBED//g" \
            | split_embeds \
            | grep -oE "$EMBED_OPEN"'.{0,80}' \
            | grep -vE "$EMBED_OPEN"' *"' \
            | grep -vE 'concat! *\( *env! *\( *"OUT_DIR"' \
            | sed "s|^|  $f: |"
    done
)"
if [ -n "$UNRECOGNISED" ]; then
    echo "UNRECOGNISED embed form - cannot prove it is not excluded:"
    printf '%s\n' "$UNRECOGNISED"
    drift=1
fi

# A silent zero would mean the grep stopped matching and the guard is dead.
if [ "$checked" -eq 0 ]; then
    echo "no embedded files found - this guard is not working"
    exit 1
fi

if [ "$drift" -ne 0 ]; then
    echo
    echo "Fix by restoring the missing file, narrowing the pathspec in $CONF,"
    echo "or rewriting the embed so its path is a literal this can resolve. This is"
    echo "a text scan: a comment or string spelling the macro with its '(' trips it"
    echo "too, so reword those."
    exit 1
fi

echo "PASS: $checked embedded file(s), none excluded"

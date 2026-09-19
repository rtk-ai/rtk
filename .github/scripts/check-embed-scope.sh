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
# shellcheck source=load-pathspecs.sh
. "$SCRIPT_DIR/load-pathspecs.sh"

cd "$SCRIPT_DIR/../.." || exit 1
CONF=.github/inert-paths.conf

load_pathspecs "$CONF" || { echo "$CONF is unreadable or lists no pathspecs"; exit 1; }
excludes=("${PATHSPECS[@]}")

# `git ls-files -- <literal> :(exclude)...` returns nothing whether or not the
# exclude matches, so membership in the surviving set is the only usable test.
kept="$(git ls-files --cached --others --exclude-standard -- ':/' "${excludes[@]}")"

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

drift=0
checked=0

# rustfmt wraps long paths onto their own line, out of reach of a
# line-oriented grep.
while IFS= read -r hit; do
    src="${hit%%|*}"
    target="${hit#*|}"

    checked=$((checked + 1))
    resolved="$(normalize_path "$(dirname "$src")/$target")"
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

    if ! printf '%s\n' "$kept" | grep -Fxq "$resolved"; then
        echo "DRIFT: $src embeds $resolved, which $CONF excludes."
        echo "       A change to it alone would skip the build and ship stale bytes."
        drift=1
    fi
done < <(
    git ls-files '*.rs' | while IFS= read -r f; do
        tr '\n' ' ' < "$f" \
            | awk '{gsub(/include_(str|bytes)! *\(/, "\n&"); print}' \
            | grep -oE 'include_(str|bytes)! *\( *"[^"]+"' \
            | sed -E 's/.*"([^"]+)"$/'"$(printf '%s' "$f" | sed 's/[&/\]/\\&/g')"'|\1/'
    done | sort -u
)

# A literal that does not follow the macro directly cannot be resolved here, so
# it has to be named rather than passed over -- otherwise an embed added as
# include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/x.md")) would be
# invisible to this guard, which is the one thing standing between the inert
# list and a stale binary.
#
# The awk split above puts each macro on its own line first. grep -o does not
# match overlapping windows, so without it the 80 characters trailing a literal
# embed swallow whichever embed follows it, and that one is never reported.
UNRECOGNISED="$(
    git ls-files '*.rs' | while IFS= read -r f; do
        tr '\n' ' ' < "$f" \
            | awk '{gsub(/include_(str|bytes)! *\(/, "\n&"); print}' \
            | grep -oE 'include_(str|bytes)! *\(.{0,80}' \
            | grep -vE 'include_(str|bytes)! *\( *"' \
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
    echo "or rewriting the embed so its path is a literal this can resolve."
    exit 1
fi

echo "PASS: $checked embedded file(s), none excluded"

#!/usr/bin/env bash
# Build the raw/rtk output pairs for the Perl A/B check. See README.md in this directory.
#
# Usage: scripts/perl-ab/build-scenarios.sh [OUT_DIR] [--network]
#
# Copies sample/ to a temp dir, runs each scenario twice (raw through `rtk proxy`, filtered
# through `rtk`), and writes OUT_DIR/<scenario>.raw.txt, <scenario>.rtk.txt and exit_codes.txt.
# Both captures include stderr (2>&1), which is what an agent's shell tool shows.
# Scenarios whose tool is not installed are skipped and listed in skipped.txt.
# --network adds the cpanm scenario, which downloads from CPAN into the temp dir.
set -uo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:-$HERE/out}
NETWORK=0
for arg in "$@"; do
    [ "$arg" = "--network" ] && NETWORK=1
done
RTK=${RTK:-rtk}

mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
: >"$OUT/exit_codes.txt"
: >"$OUT/skipped.txt"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fresh_copy() {
    rm -rf "$WORK/Acme-RtkSample"
    cp -r "$HERE/sample" "$WORK/Acme-RtkSample"
    rm -f "$WORK/Acme-RtkSample/Makefile.PL.cover"
    cd "$WORK/Acme-RtkSample" || exit 1
}

# Optional command run before each side of a scenario, so a scenario that changes the copy
# (a build dir, installed modules, a coverage database) gives both sides the same start.
SETUP=""

# scenario NAME TOOL ARGS... : run `TOOL ARGS` raw and through rtk.
scenario() {
    local name=$1 tool=$2
    shift 2
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$name ($tool not installed)" >>"$OUT/skipped.txt"
        return
    fi
    [ -n "$SETUP" ] && $SETUP
    "$RTK" proxy "$tool" "$@" >"$OUT/$name.raw.txt" 2>&1
    local raw_exit=$?
    [ -n "$SETUP" ] && $SETUP
    "$RTK" "$tool" "$@" >"$OUT/$name.rtk.txt" 2>&1
    local rtk_exit=$?
    echo "$name raw=$raw_exit rtk=$rtk_exit" >>"$OUT/exit_codes.txt"
}

fresh_copy
scenario prove_fail prove -l t
scenario prove_fail_jobs prove -l -j4 t
scenario prove_bailout prove -l t-bail
scenario prove_pass prove -l t-pass
scenario yath_fail yath test -Ilib t
scenario perlcritic_sev1 perlcritic --noprofile --severity 1 lib
scenario perldoc_function perldoc -f sprintf

SETUP=fresh_copy
scenario dzil_test dzil test

cover_copy() {
    fresh_copy
    cp "$HERE/sample/Makefile.PL.cover" Makefile.PL
    perl Makefile.PL >/dev/null 2>&1
}
SETUP=cover_copy
scenario cover_test cover -test
# The text report reads the database cover -test wrote, so it keeps that copy.
SETUP=""
scenario cover_report cover -report text

if [ "$NETWORK" = 1 ]; then
    SETUP=fresh_copy
    scenario cpanm_installdeps cpanm -L local --installdeps .
    SETUP=""
else
    echo "cpanm_installdeps (needs --network)" >>"$OUT/skipped.txt"
fi

cp "$HERE/questions.md" "$OUT/questions.md"
echo "Wrote $(ls "$OUT"/*.rtk.txt 2>/dev/null | wc -l) scenario pairs to $OUT"
[ -s "$OUT/skipped.txt" ] && { echo "Skipped:"; cat "$OUT/skipped.txt"; }
exit 0

#!/bin/sh
# Records ../demo.gif: a real session, nothing scripted but the typing.
#
#   WORK=/some/empty/dir CRATE=/path/to/a/crate/with/failing/tests ./record-demo.sh
#
# Needs asciinema, agg, a release build of rtk at ../../../target/release/rtk,
# and a logged-in Claude Code. The session runs in an empty HOME ($WORK/home)
# so it starts from no RTK at all and `rtk gain` shows only what it did; the
# shim in $WORK/bin points `rtk` at the release build inside that HOME. Claude
# is told to load only the hook `rtk init -g` just wrote, nothing from your own
# settings. That HOME's path is shown as ~ in the output, which is the only
# change made to what the commands printed. The shim also sets
# RTK_TELEMETRY_DISABLED=1, so `rtk init -g` asks no telemetry question here.
set -e
WORK="${WORK:?set WORK to an empty directory}"
CRATE="${CRATE:?set CRATE to the crate the agent will test}"
HERE="$(cd "$(dirname "$0")" && pwd)"
RTK="$HERE/../../../target/release/rtk"
[ -x "$RTK" ] || { echo "build rtk first: cargo build --release" >&2; exit 1; }
mkdir -p "$WORK/home" "$WORK/bin"
cat > "$WORK/bin/rtk" <<EOF
#!/bin/sh
unset RTK_DB_PATH RTK_CONFIG_DIR RTK_TEE_DIR RTK_RECALL_DB
HOME="$WORK/home" RTK_TELEMETRY_DISABLED=1 exec "$RTK" "\$@"
EOF
chmod +x "$WORK/bin/rtk"
printf 'export PATH="%s/bin:$PATH"\n' "$WORK" > "$WORK/env.sh"
cat > "$WORK/pathfilter.py" <<'PY'
import sys
pat = sys.argv[1].encode(); rep = sys.argv[2].encode()
out = sys.stdout.buffer; inp = sys.stdin.buffer; buf = b""
while True:
    c = inp.read(1)
    if not c:
        out.write(buf); out.flush(); break
    buf += c
    if buf == pat:
        out.write(rep); out.flush(); buf = b""
    elif not pat.startswith(buf):
        out.write(buf); out.flush(); buf = b""
PY
cat > "$WORK/session.sh" <<SH
#!/bin/sh
export PATH="$WORK/bin:\$PATH"
cd "$CRATE" || exit 1
type_cmd() { printf '\033[1;32m\$\033[0m '; python3 -I -c 'import sys,time
for c in sys.argv[1]:
    sys.stdout.write(c); sys.stdout.flush(); time.sleep(0.04)
print()' "\$1"; sleep 0.6; }
sleep 1
type_cmd "rtk init -g"
(sleep 1.5; echo y; sleep 1.4; echo n; sleep 1.2; echo n; sleep 1.5) | script -q /dev/null rtk init -g 2>&1 | python3 -I "$WORK/pathfilter.py" "$WORK/home" "~"
sleep 1.5
type_cmd 'claude -p "Run the test suite and tell me what fails."'
CLAUDE_ENV_FILE="$WORK/env.sh" claude -p "Run the test suite and tell me what fails." --model sonnet --setting-sources project --settings "$WORK/home/.claude/settings.json" --append-system-prompt "\$(cat "$WORK/home/.claude/RTK.md")" --strict-mcp-config --no-session-persistence --allowedTools Bash 2>&1
sleep 2
type_cmd "rtk gain"
rtk gain 2>&1
sleep 4
SH
TERM=xterm-256color asciinema rec --overwrite --cols 100 --rows 30 --idle-time-limit 3 -c "sh $WORK/session.sh" "$WORK/demo.cast"
agg --cols 100 --rows 30 --font-size 15 --speed 1.25 \
  --theme 060b18,e2e8f0,060b18,f87171,00e599,fbbf24,60a5fa,c084fc,22d3ee,e2e8f0,7e8ca6,fca5a5,5eead4,fde68a,93c5fd,d8b4fe,67e8f9,f8fafc \
  "$WORK/demo.cast" "$HERE/../demo.gif"
echo "$HERE/../demo.gif"

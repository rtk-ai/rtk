#!/bin/sh
[ "$(uname -s)" = Linux ] && [ -d /home/claude ] || exit 0
command -v python3 >/dev/null 2>&1 || exit 0
exec python3 "${CLAUDE_PLUGIN_ROOT}/hooks/summary.py" "$1"

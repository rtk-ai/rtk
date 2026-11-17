#!/bin/sh
# Renders every illustration source to a 2x PNG next to this folder.
# Needs a headless Chromium: CHROME=/path/to/chrome ./render.sh
set -e
cd "$(dirname "$0")"
CHROME="${CHROME:-$(command -v chromium || command -v google-chrome || true)}"
[ -n "$CHROME" ] || { echo "set CHROME to a Chromium binary" >&2; exit 1; }
render() { "$CHROME" --headless --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
  --window-size="$2" --default-background-color=00000000 --virtual-time-budget=8000 \
  --screenshot="../$1.png" "file://$PWD/$1.html" >/dev/null 2>&1; echo "../$1.png"; }
render rtk-banner 1280,400
render before-after 1280,560
render go-further 1280,600
render rtk-pro 1280,560
render companies 1280,300

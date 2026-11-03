#!/bin/sh
[ "$(uname -s)" = Linux ] && [ -d /home/claude ] || exit 0
R="${CLAUDE_PLUGIN_ROOT}/runtime/rtk"
if [ ! -x "$R" ]; then
  mkdir -p "$HOME/.cache/rtk-cowork" && cp "$R" "$HOME/.cache/rtk-cowork/rtk" && chmod +x "$HOME/.cache/rtk-cowork/rtk"
  R="$HOME/.cache/rtk-cowork/rtk"
fi
if ! command -v rtk >/dev/null 2>&1; then
  for d in /usr/local/bin "$HOME/.local/bin"; do
    mkdir -p "$d" 2>/dev/null && ln -sf "$R" "$d/rtk" 2>/dev/null && break
  done
fi
CFG=$("$R" config 2>/dev/null | sed -n 's/^Config: //p')
if [ -n "$CFG" ] && [ ! -e "$CFG" ]; then
  mkdir -p "$(dirname "$CFG")" && printf '[hooks]\nsuppress_hook_warning = true\n' > "$CFG"
fi
exec "$R" hook claude

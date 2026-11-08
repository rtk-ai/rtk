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
IN=$(cat)
if ! command -v rtk >/dev/null 2>&1; then
  case "$IN" in *PreToolUse*) exit 0 ;; esac
fi
STATE_DIR="${RTK_COWORK_STATE:-/tmp/rtk-cowork}"
if [ -z "$RTK_CONFIG_DIR" ] && [ "$(cat "$STATE_DIR/pipeline" 2>/dev/null)" = off ]; then
  export RTK_CONFIG_DIR="$STATE_DIR/cfg-off"
fi

if [ -n "$RTK_CONFIG_DIR" ]; then
  CFG="$RTK_CONFIG_DIR/config.toml"
else
  CFG=$("$R" config 2>/dev/null | sed -n 's/^Config: //p')
fi
if [ -n "$CFG" ] && [ ! -e "$CFG" ]; then
  mkdir -p "$(dirname "$CFG")" && printf '[hooks]\nsuppress_hook_warning = true\n' > "$CFG"
fi
printf '%s' "$IN" | "$R" hook claude

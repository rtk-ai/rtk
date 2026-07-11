#!/bin/bash
# Install srtk (rtk with PII redaction) and wire it into Claude Code.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/slice-ravichopra/rtk/feat/pii-redaction/scripts/install-srtk.sh | bash
#
# What it does:
#   1. Downloads the srtk binary (Apple Silicon) to ~/.local/bin/srtk
#   2. Installs the Claude Code rewrite hook to ~/.claude/hooks/rtk-rewrite.sh
#   3. Registers the hook and renames rtk permission rules to srtk in
#      ~/.claude/settings.json (backup written first)
#   4. Adds an "always use srtk" note to ~/.claude/RTK.md
#
# Safe to re-run (idempotent).

set -euo pipefail

REPO="slice-ravichopra/rtk"
TAG="${SRTK_TAG:-v0.42.4-pii.1}"
ASSET="srtk-darwin-arm64"
BIN="$HOME/.local/bin/srtk"
HOOK="$HOME/.claude/hooks/rtk-rewrite.sh"
SETTINGS="$HOME/.claude/settings.json"

if [ "$(uname -sm)" != "Darwin arm64" ]; then
  echo "error: prebuilt binary is Apple Silicon only — build from source:" >&2
  echo "  git clone git@github.com:$REPO.git && cd rtk && cargo build --release" >&2
  echo "  cp target/release/rtk $BIN" >&2
  exit 1
fi

command -v jq >/dev/null || { echo "installing jq..."; brew install jq; }

echo "==> downloading srtk $TAG"
mkdir -p "$(dirname "$BIN")"
curl -fsSL -o "$BIN" "https://github.com/$REPO/releases/download/$TAG/$ASSET"
chmod +x "$BIN"
"$BIN" --version

echo "==> smoke test"
OUT=$("$BIN" proxy sh -c 'echo probe a@b.co card 4111-1111-1111-1111')
echo "$OUT" | grep -q 'REDACTED:email' || { echo "error: redaction not working: $OUT" >&2; exit 1; }
echo "    $OUT"

echo "==> installing Claude Code hook"
mkdir -p "$(dirname "$HOOK")"
# Embedded inline (kept in sync with scripts/claude-srtk-hook.sh) so a single
# fetch installs everything — no second download that could version-skew.
cat >"$HOOK" <<'HOOK_EOF'
#!/bin/bash
# Claude Code PreToolUse hook: rewrite Bash commands to their srtk equivalent.
# srtk = rtk build with PII redaction (slice-ravichopra/rtk, feat/pii-redaction).
# Only rewrites the command text — permission checks still apply to the
# rewritten command as usual (no permissionDecision is emitted).
# Exits 0 with no output → command runs unchanged.

SRTK="$HOME/.local/bin/srtk"
[ -x "$SRTK" ] || exit 0

JQ="$(command -v jq)"
[ -n "$JQ" ] || exit 0

INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | "$JQ" -r '.tool_input.command // empty' 2>/dev/null)
[ -n "$CMD" ] || exit 0

# Never rewrite commands already using rtk/srtk.
case "$CMD" in
  srtk\ *|rtk\ *|*"/srtk "*|*"/rtk "*) exit 0 ;;
esac

# srtk rewrite exit codes: 0 = rewritten (allow-listed), 3 = rewritten (no
# allow rule yet — Claude will ask as usual), 1 = no equivalent, 2 = deny.
REWRITTEN=$("$SRTK" rewrite "$CMD" 2>/dev/null)
rc=$?
{ [ "$rc" -eq 0 ] || [ "$rc" -eq 3 ]; } || exit 0
[ -n "$REWRITTEN" ] || exit 0

# srtk emits "rtk ..." — force the srtk binary name.
REWRITTEN="srtk ${REWRITTEN#rtk }"

printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":%s}}}' \
  "$(printf '%s' "$REWRITTEN" | "$JQ" -Rs .)"
HOOK_EOF
chmod +x "$HOOK"

if [ -f "$SETTINGS" ]; then
  cp "$SETTINGS" "$SETTINGS.bak-srtk"
  echo "==> updating $SETTINGS (backup: $SETTINGS.bak-srtk)"
  # Rename rtk permission rules to srtk.
  tmp=$(mktemp)
  sed -e 's/Bash(rtk /Bash(srtk /g' -e 's/Bash(AWS_PROFILE=\* rtk /Bash(AWS_PROFILE=* srtk /g' \
    "$SETTINGS" >"$tmp"
  # Register the PreToolUse hook if not present.
  jq --arg hook "$HOOK" '
    .hooks.PreToolUse = ((.hooks.PreToolUse // []) as $p
      | if ($p | map(select(.hooks[]?.command == $hook)) | length) > 0 then $p
        else $p + [{"matcher":"Bash","hooks":[{"type":"command","command":$hook}]}] end)
  ' "$tmp" >"$SETTINGS"
  rm -f "$tmp"
  jq empty "$SETTINGS" || { echo "error: settings.json broken — restoring backup" >&2; cp "$SETTINGS.bak-srtk" "$SETTINGS"; exit 1; }
else
  echo "==> creating $SETTINGS with hook registration"
  mkdir -p "$(dirname "$SETTINGS")"
  jq -n --arg hook "$HOOK" \
    '{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":$hook}]}]}}' \
    >"$SETTINGS"
fi

RTK_MD="$HOME/.claude/RTK.md"
if [ ! -f "$RTK_MD" ] || ! grep -q '## srtk — PII Redaction' "$RTK_MD"; then
  echo "==> adding srtk note to $RTK_MD"
  cat >>"$RTK_MD" <<'EOF'

## srtk — PII Redaction (IMPORTANT)

**Always use `srtk`, never plain `rtk`.** srtk masks PII (emails, phones,
PAN, Aadhaar, card numbers, AWS keys, IPs) as `[REDACTED:<category>]` before
output reaches the LLM — including `srtk proxy`. Disable per run with
`--no-redact` (only when the user asks). Details:
https://github.com/slice-ravichopra/rtk/blob/feat/pii-redaction/docs/redaction.md
EOF
fi

echo
echo "✅ srtk installed. Restart Claude Code sessions to pick up the hook."

#!/usr/bin/env bash
# rtk-hooks.sh — Unified RTK hook for Claude Code
#
# Handles two hook events:
#   PreToolUse:Bash  → Classify & rewrite unknown CLI commands through RTK
#   PostToolUse:mcp__* → Source-level trimming on large MCP tool outputs
#
# Both CLI fallback and MCP output use the same source-level trimming logic:
#   - Strip blank lines (collapse consecutive blanks to zero)
#   - Strip decorative separators (═╔╗╚╝║─=-#*~ 3+ chars)
#   - Strip banners ([ok] Command: ...)
#   - Strip list count lines (3 items, 15 entries, etc.)
#   - Strip markdown header prefix (# ## ### etc.) — keep the text
#   - Strip bullet prefix (•·▪▸▹➤) — keep the text
#   NO content reformatting, NO truncation, NO key=value compaction.
#
# Installation in ~/.claude/settings.json:
#   {
#     "hooks": {
#       "PreToolUse": [
#         { "matcher": "Bash", "hooks": [{ "type": "command", "command": "bash ~/.claude/hooks/rtk-hooks.sh pre-tool-use" }] }
#       ],
#       "PostToolUse": [
#         { "matcher": "mcp__*", "hooks": [{ "type": "command", "command": "bash ~/.claude/hooks/rtk-hooks.sh post-tool-use" }] }
#       ]
#     }
#   }
#
# Requires: bash, jq, rtk (optional — falls back gracefully)

set -euo pipefail

# ── Constants ──────────────────────────────────────────────────────────────

MODE="${1:-}"

# RTK-native commands → rtk rewrite (dedicated high-quality filters)
RTK_NATIVE="git ls tree find cat wc grep rg diff pnpm npm npx cargo pip docker kubectl aws psql curl wget jest vitest pytest rspec playwright tsc eslint prettier ruff mypy rubocop gh glab gt go"

# Build/compile/test commands → rtk err (success=silent, failure=errors only)
ERR_WRAP="mvn gradle gradlew ant sbt lein boot make cmake ninja bazel buck pants cucumber karma mocha flake8 pylint golangci-lint"

# Internal CLI tools → rtk fallback (source-level trimming: blank lines, banners, separators)
# These go through RTK's fallback path which applies filter_fallback_output()
FALLBACK_WRAP="sofam aiag-cli dima apc"

# Trivial commands → skip (no benefit to wrap)
SKIP_CMDS="echo printf which type export source cd pwd true false exit set unset alias unalias history clear reset rtk claude date uname hostname id whoami env printenv test"

# ── Guards ─────────────────────────────────────────────────────────────────

if ! command -v jq &>/dev/null; then
  exit 0
fi

# ── Helper: source-level trim (matches Rust filter_fallback_output) ───────
#
# Strips blank lines, decorative separators, banners, markdown headers,
# bullet prefixes, and list count lines. Does NOT reformat or truncate content.

_source_level_trim() {
  local text
  text=$(cat)

  [[ -z "$text" ]] && { echo ""; return; }

  local result=()
  local prev_blank=0

  while IFS= read -r line; do
    local trimmed="${line#"${line%%[![:space:]]*}"}"
    trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"

    # Skip blank lines (collapse consecutive blanks to zero)
    if [[ -z "$trimmed" ]]; then
      if [[ $prev_blank -eq 0 && ${#result[@]} -gt 0 ]]; then
        prev_blank=1
      fi
      continue
    fi

    # Skip decorative separators (═╔╗╚╝║─=-#*~ 3+ chars)
    local is_separator=0
    echo "$trimmed" | grep -qxE '[═╔╗╚╝║─=#*~\-]{3,}[[:space:]]*' 2>/dev/null && is_separator=1 || true
    if [[ $is_separator -eq 1 ]]; then
      continue
    fi

    # Skip banners like "[ok] Command: ..."
    local is_banner=0
    echo "$trimmed" | grep -qxE '\[ok\][[:space:]]+Command:.*' 2>/dev/null && is_banner=1 || true
    if [[ $is_banner -eq 1 ]]; then
      continue
    fi

    # Skip list count lines like "3 items" or "15 entries"
    local is_list_count=0
    echo "$trimmed" | grep -qxE '[0-9]+[[:space:]]+(items?|entries?|rows?|results?|records?|lines?)[[:space:]]*' 2>/dev/null && is_list_count=1 || true
    if [[ $is_list_count -eq 1 ]]; then
      continue
    fi

    prev_blank=0

    # Strip markdown header prefix (# ## ### etc.) — keep the text
    local is_header=0
    echo "$line" | grep -qxE '#{1,6}[[:space:]].*' 2>/dev/null && is_header=1 || true
    if [[ $is_header -eq 1 ]]; then
      local cleaned
      cleaned=$(echo "$line" | sed -E 's/^#{1,6}[[:space:]]+//')
      result+=("$cleaned")
      continue
    fi

    # Strip bullet prefix (•·▪▸▹➤) — keep the text
    local is_bullet=0
    echo "$line" | grep -qE '^[[:space:]]*[•·▪▸▹➤][[:space:]]' 2>/dev/null && is_bullet=1 || true
    if [[ $is_bullet -eq 1 ]]; then
      local cleaned
      cleaned=$(echo "$line" | sed -E 's/^[[:space:]]*[•·▪▸▹➤][[:space:]]*//')
      result+=("$cleaned")
      continue
    fi

    result+=("$line")
  done <<< "$text"

  # Join lines with newline
  local first=1
  for line in "${result[@]}"; do
    if [[ $first -eq 1 ]]; then
      printf '%s' "$line"
      first=0
    else
      printf '\n%s' "$line"
    fi
  done
}

# ── Helper: classify a single command segment ──────────────────────────────

classify_segment() {
  local cmd="$1"
  local stripped="${cmd#"${cmd%%[![:space:]]*}"}"  # ltrim
  stripped="${stripped%"${stripped##*[![:space:]]}"}"  # rtrim

  if [[ -z "$stripped" ]]; then
    echo "skip"
    return
  fi

  # Already wrapped with rtk
  if [[ "$stripped" == rtk\ * ]]; then
    echo "skip"
    return
  fi

  # Extract base command
  local first="${stripped%% *}"
  local base="${first##*/}"  # strip path prefix

  # Check skip list
  local skip_cmd
  for skip_cmd in $SKIP_CMDS; do
    if [[ "$base" == "$skip_cmd" ]]; then
      echo "skip"
      return
    fi
  done

  # Check RTK-native → try rtk rewrite
  local native_cmd
  for native_cmd in $RTK_NATIVE; do
    if [[ "$base" == "$native_cmd" ]]; then
      # Try rtk rewrite
      if command -v rtk &>/dev/null; then
        local rewritten=""
        local exit_code=0
        rewritten=$(rtk rewrite "$stripped" 2>/dev/null) || exit_code=$?
        if [[ $exit_code -eq 0 && -n "$rewritten" && "$rewritten" != "$stripped" ]]; then
          echo "rewrite:$rewritten"
          return
        elif [[ $exit_code -eq 2 ]]; then
          echo "deny"
          return
        elif [[ $exit_code -eq 3 && -n "$rewritten" ]]; then
          echo "ask:$rewritten"
          return
        fi
      fi
      # Fall through to summary if rewrite fails
      echo "summary:rtk summary $stripped"
      return
    fi
  done

  # Check err-wrap list
  local err_cmd
  for err_cmd in $ERR_WRAP; do
    if [[ "$base" == "$err_cmd" ]]; then
      echo "err:rtk err $stripped"
      return
    fi
  done

  # Check fallback-wrap list → rtk <cmd> (source-level trimming via fallback path)
  local fb_cmd
  for fb_cmd in $FALLBACK_WRAP; do
    if [[ "$base" == "$fb_cmd" ]]; then
      echo "fallback:rtk $stripped"
      return
    fi
  done

  # Everything else → summary
  echo "summary:rtk summary $stripped"
}

# ── PreToolUse:Bash handler ───────────────────────────────────────────────

handle_pre_tool_use() {
  local input
  input=$(cat)

  local tool_name
  tool_name=$(echo "$input" | jq -r '.tool_name // empty')
  if [[ "$tool_name" != "Bash" ]]; then
    exit 0
  fi

  local cmd
  cmd=$(echo "$input" | jq -r '.tool_input.command // empty')
  if [[ -z "$cmd" ]]; then
    exit 0
  fi

  # Skip already-wrapped or trivial single commands
  local first_word="${cmd%% *}"
  local base="${first_word##*/}"
  local skip_cmd
  for skip_cmd in $SKIP_CMDS; do
    if [[ "$base" == "$skip_cmd" ]]; then
      exit 0
    fi
  done
  if [[ "$cmd" == rtk\ * ]]; then
    exit 0
  fi

  # Check if compound command (contains &&, ||, ;)
  local is_compound=0
  if [[ "$cmd" == *"&&"* ]] || [[ "$cmd" == *"||"* ]] || [[ "$cmd" == *";"* ]]; then
    is_compound=1
  fi

  if [[ $is_compound -eq 1 ]]; then
    # Split on operators, classify each segment
    local rewritten_cmd=""
    local has_deny=0
    local has_ask=0
    local any_changed=0

    # Split on &&, ||, ; keeping delimiters as separate tokens
    local IFS_SAVE="$IFS"
    local segments=()
    local ops=()
    local current=""

    # Simple approach: iterate and split
    local remaining="$cmd"
    while [[ -n "$remaining" ]]; do
      # Find the earliest operator
      local and_pos=-1 or_pos=-1 semi_pos=-1
      local next_op="" next_pos=-1

      if [[ "$remaining" == *"&&"* ]]; then
        and_pos=${remaining%%&&*}
        and_pos=${#and_pos}
      fi
      if [[ "$remaining" == *"||"* ]]; then
        or_pos=${remaining%%||*}
        or_pos=${#or_pos}
      fi
      if [[ "$remaining" == *";"* ]]; then
        semi_pos=${remaining%%;*}
        semi_pos=${#semi_pos}
      fi

      # Find minimum positive position
      local min_pos=-1
      local min_op=""
      if [[ $and_pos -ge 0 ]] && { [[ $min_pos -lt 0 ]] || [[ $and_pos -lt $min_pos ]]; }; then
        min_pos=$and_pos; min_op="&&"
      fi
      if [[ $or_pos -ge 0 ]] && { [[ $min_pos -lt 0 ]] || [[ $or_pos -lt $min_pos ]]; }; then
        min_pos=$or_pos; min_op="||"
      fi
      if [[ $semi_pos -ge 0 ]] && { [[ $min_pos -lt 0 ]] || [[ $semi_pos -lt $min_pos ]]; }; then
        min_pos=$semi_pos; min_op=";"
      fi

      if [[ $min_pos -lt 0 ]]; then
        # No more operators, rest is a segment
        segments+=("$remaining")
        break
      fi

      # Segment before operator
      local seg="${remaining:0:$min_pos}"
      segments+=("$seg")
      ops+=("$min_op")

      # Skip past operator
      local op_len=${#min_op}
      remaining="${remaining:$((min_pos + op_len))}"
    done

    # Classify each segment
    local i=0
    for seg in "${segments[@]}"; do
      local result
      result=$(classify_segment "$seg")

      if [[ "$result" == "skip" ]]; then
        rewritten_cmd+="$seg"
      elif [[ "$result" == "deny" ]]; then
        has_deny=1
        rewritten_cmd+="$seg"
      elif [[ "$result" == ask:* ]]; then
        has_ask=1
        any_changed=1
        local rw="${result#ask:}"
        rewritten_cmd+="$rw"
      else
        any_changed=1
        # result is "rewrite:...", "err:...", "summary:...", or "fallback:..."
        local rw="${result#*:}"
        rewritten_cmd+="$rw"
      fi

      # Add operator if present
      if [[ $i -lt ${#ops[@]} ]]; then
        rewritten_cmd+=" ${ops[$i]} "
      fi
      i=$((i + 1))
    done

    # If nothing changed, passthrough
    if [[ $any_changed -eq 0 && $has_deny -eq 0 ]]; then
      exit 0
    fi

    # Build output
    local original_input
    original_input=$(echo "$input" | jq -c '.tool_input')

    if [[ $has_deny -eq 1 ]]; then
      jq -n \
        '{
          "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": "RTK deny rule"
          }
        }'
    elif [[ $has_ask -eq 1 ]]; then
      local updated_input
      updated_input=$(echo "$original_input" | jq --arg cmd "$rewritten_cmd" '.command = $cmd')
      jq -n \
        --argjson updated "$updated_input" \
        '{
          "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": $updated
          }
        }'
    else
      local updated_input
      updated_input=$(echo "$original_input" | jq --arg cmd "$rewritten_cmd" '.command = $cmd')
      jq -n \
        --argjson updated "$updated_input" \
        '{
          "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": $updated
          }
        }'
    fi
    return
  fi

  # ── Single command (not compound) ──
  local result
  result=$(classify_segment "$cmd")

  if [[ "$result" == "skip" ]]; then
    exit 0
  fi

  local original_input
  original_input=$(echo "$input" | jq -c '.tool_input')

  if [[ "$result" == "deny" ]]; then
    jq -n \
      '{
        "hookSpecificOutput": {
          "hookEventName": "PreToolUse",
          "permissionDecision": "deny",
          "permissionDecisionReason": "RTK deny rule"
        }
      }'
  elif [[ "$result" == ask:* ]]; then
    local rw="${result#ask:}"
    local updated_input
    updated_input=$(echo "$original_input" | jq --arg cmd "$rw" '.command = $cmd')
    jq -n \
      --argjson updated "$updated_input" \
      '{
        "hookSpecificOutput": {
          "hookEventName": "PreToolUse",
          "updatedInput": $updated
        }
      }'
  else
    # rewrite, err, summary, or fallback
    local rw="${result#*:}"
    local action="${result%%:*}"
    local reason="RTK ${action}"
    local updated_input
    updated_input=$(echo "$original_input" | jq --arg cmd "$rw" '.command = $cmd')
    jq -n \
      --argjson updated "$updated_input" \
      --arg reason "$reason" \
      '{
        "hookSpecificOutput": {
          "hookEventName": "PreToolUse",
          "permissionDecision": "allow",
          "permissionDecisionReason": $reason,
          "updatedInput": $updated
        }
      }'
  fi
}

# ── PostToolUse:mcp__* handler ────────────────────────────────────────────

handle_post_tool_use() {
  local input
  input=$(cat)

  local tool_name
  tool_name=$(echo "$input" | jq -r '.tool_name // empty')

  # Only process MCP tool outputs
  if [[ "$tool_name" != mcp__* ]]; then
    exit 0
  fi

  # Extract tool_result as string
  local tool_result
  tool_result=$(echo "$input" | jq -r '.tool_result // empty')

  # Nothing to trim
  if [[ -z "$tool_result" ]]; then
    exit 0
  fi

  # Apply source-level trim (same logic as Rust filter_fallback_output)
  local trimmed
  trimmed=$(printf '%s' "$tool_result" | _source_level_trim)

  # If trimmed is identical to original, passthrough
  if [[ "$trimmed" == "$tool_result" ]]; then
    exit 0
  fi

  # Update tool_result with trimmed version
  local tool_result_type
  tool_result_type=$(echo "$input" | jq -r '.tool_result | type')

  if [[ "$tool_result_type" == "string" ]]; then
    echo "$input" | jq --arg trimmed "$trimmed" '.tool_result = $trimmed'
  elif [[ "$tool_result_type" == "object" || "$tool_result_type" == "array" ]]; then
    # For structured results, convert to string, trim, and put back as string
    local json_str
    json_str=$(echo "$input" | jq -c '.tool_result')
    local trimmed_json
    trimmed_json=$(echo "$json_str" | _source_level_trim)
    echo "$input" | jq --arg trimmed "$trimmed_json" '.tool_result = $trimmed'
  else
    # Unknown type — passthrough
    exit 0
  fi
}

# ── Main dispatch ─────────────────────────────────────────────────────────

case "$MODE" in
  pre-tool-use)
    handle_pre_tool_use
    ;;
  post-tool-use)
    handle_post_tool_use
    ;;
  *)
    # Unknown mode — passthrough
    exit 0
    ;;
esac
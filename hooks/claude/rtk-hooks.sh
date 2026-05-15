#!/usr/bin/env bash
# rtk-hooks.sh — Unified RTK hook for Claude Code
#
# Handles two hook events:
#   PreToolUse:Bash  → Classify & rewrite unknown CLI commands through RTK
#   PostToolUse:mcp__* → Compress large MCP tool outputs
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

# Maximum MCP output characters before compression (approx 2000 tokens)
MAX_OUTPUT_CHARS=8000
# Maximum characters for a single string value in structured JSON
MAX_VALUE_CHARS=2000
# Maximum array items before truncation
MAX_ARRAY_ITEMS=50
MAX_ARRAY_KEEP=30
# Minimum compression ratio to accept rtk pipe result
MIN_COMPRESSION_RATIO=0.85

# ── Guards ─────────────────────────────────────────────────────────────────

if ! command -v jq &>/dev/null; then
  exit 0
fi

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
    # Use sed to split while preserving operators as separate tokens
    local rewritten_cmd=""
    local has_deny=0
    local has_ask=0
    local any_changed=0

    # Split on &&, ||, ; keeping delimiters
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
        # result is "rewrite:...", "err:...", or "summary:..."
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
    # rewrite, err, or summary
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

  local tool_result_len
  tool_result_len=$(echo "$input" | jq -r '.tool_result // empty' | wc -c | tr -d ' ')

  # Nothing to compress
  if [[ "$tool_result_len" -le "$MAX_OUTPUT_CHARS" ]]; then
    exit 0
  fi

  # Extract tool_result as string
  local tool_result_type
  tool_result_type=$(echo "$input" | jq -r '.tool_result | type')

  if [[ "$tool_result_type" == "string" ]]; then
    # String result — try rtk pipe first, then smart truncation
    local compressed=""
    if command -v rtk &>/dev/null; then
      compressed=$(echo "$input" | jq -r '.tool_result' | timeout 1 rtk pipe 2>/dev/null) || true
      if [[ -n "$compressed" ]]; then
        local comp_len=${#compressed}
        local orig_len=$((tool_result_len - 1))  # wc -c includes newline
        if [[ $orig_len -gt 0 ]] && [[ $(echo "scale=2; $comp_len / $orig_len" | bc) < $MIN_COMPRESSION_RATIO ]]; then
          # Good compression — use it
          echo "$input" | jq --arg compressed "$compressed" '.tool_result = $compressed'
          return
        fi
      fi
    fi

    # Fallback: smart truncation preserving important lines
    local truncated
    truncated=$(echo "$input" | jq -r '.tool_result' | _smart_truncate)
    echo "$input" | jq --arg truncated "$truncated" '.tool_result = $truncated'
    return

  elif [[ "$tool_result_type" == "object" || "$tool_result_type" == "array" ]]; then
    # Structured result — compress long string values and truncate arrays
    local compressed_json
    compressed_json=$(echo "$input" | jq -r '.tool_result' | _compress_json_value)
    echo "$input" | jq --argjson compressed "$compressed_json" '.tool_result = $compressed'
    return
  fi

  # Unknown type — passthrough
  exit 0
}

# ── Smart truncation: preserve error/warn/success lines ────────────────────

_smart_truncate() {
  local text
  text=$(cat)

  if [[ ${#text} -le $MAX_OUTPUT_CHARS ]]; then
    echo "$text"
    return
  fi

  # Keywords that match at line start only (after optional whitespace)
  local important_pattern='^[[:space:]]*(error|Error|ERROR|fail|Fail|FAIL|warn|Warn|WARN|warning|Warning|exception|Exception|EXCEPTION|success|Success|SUCCESS)\b'

  local important_lines=()
  local other_lines=()
  local line

  while IFS= read -r line; do
    if echo "$line" | grep -qE "$important_pattern"; then
      important_lines+=("$line")
    else
      other_lines+=("$line")
    fi
  done <<< "$text"

  local important_text
  important_text=$(printf '%s\n' "${important_lines[@]+"${important_lines[@]}"}")

  if [[ ${#important_text} -le $((MAX_OUTPUT_CHARS * 8 / 10)) ]]; then
    # Show important lines + truncated other lines
    local budget=$((MAX_OUTPUT_CHARS - ${#important_text} - 100))
    local other_text
    other_text=$(printf '%s\n' "${other_lines[@]+"${other_lines[@]}"}" | head -c "$budget")
    local omitted=$(( ${#other_lines[@]} - $(echo "$other_text" | wc -l | tr -d ' ') ))
    printf '%s\n\n... [truncated: %d lines omitted] ...\n\n%s\n' "$other_text" "$omitted" "$important_text"
  else
    # Last resort: head + tail
    local head_size=$((MAX_OUTPUT_CHARS / 2))
    local tail_size=$((MAX_OUTPUT_CHARS / 4))
    printf '%s\n\n... [truncated: %d chars omitted] ...\n\n%s\n' \
      "${text:0:$head_size}" \
      "$(( ${#text} - head_size - tail_size ))" \
      "${text: -$tail_size}"
  fi
}

# ── Recursive JSON value compression ──────────────────────────────────────

_compress_json_value() {
  # Reads JSON from stdin, recursively compresses long string values
  # and truncates large arrays. Outputs compressed JSON.
  local json
  json=$(cat)

  # Check total size
  local json_len=${#json}
  if [[ $json_len -le $MAX_OUTPUT_CHARS ]]; then
    echo "$json"
    return
  fi

  # Use jq to recursively process:
  # 1. If string > MAX_VALUE_CHARS, truncate
  # 2. If array > MAX_ARRAY_ITEMS, keep first MAX_ARRAY_KEEP + notice
  echo "$json" | jq --arg max_val "$MAX_VALUE_CHARS" --arg max_items "$MAX_ARRAY_ITEMS" --arg max_keep "$MAX_ARRAY_KEEP" '
    def truncate_str:
      if length > ($max_val | tonumber) then
        .[0:($max_val | tonumber)] + "... [truncated]"
      else
        .
      end;
    def compress:
      if type == "string" then truncate_str
      elif type == "array" then
        if length > ($max_items | tonumber) then
          .[0:($max_keep | tonumber)] + ["... [truncated: \(length - ($max_keep | tonumber)) more items]"]
        else
          map(compress)
        end
      elif type == "object" then
        to_entries | map(.value |= compress) | from_entries
      else
        .
      end;
    compress
  ' 2>/dev/null || echo "$json"
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
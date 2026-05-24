#!/usr/bin/env bash

set -u

payload="$(cat)"

json_escape() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  value="${value//$'\r'/\\r}"
  value="${value//$'\t'/\\t}"
  printf '%s' "$value"
}

write_advisory() {
  local message
  message="$(json_escape "$1")"
  printf '{"systemMessage":"%s"}\n' "$message"
}

resolve_rtk() {
  if [[ -n "${RTK_EXE:-}" ]]; then
    if [[ "${RTK_EXE}" == */* ]]; then
      if [[ -x "${RTK_EXE}" ]]; then
        printf '%s\n' "${RTK_EXE}"
        return 0
      fi
      return 1
    fi

    command -v -- "${RTK_EXE}"
    return $?
  fi

  command -v rtk
}

if ! rtk_exe="$(resolve_rtk)"; then
  write_advisory "RTK Codex plugin hook could not find the rtk executable. The original Bash command will run unchanged. Set RTK_EXE or add rtk to PATH."
  exit 0
fi

printf '%s' "$payload" | "$rtk_exe" hook codex || exit 0

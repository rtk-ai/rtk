---
description: What RTK saved in this task and on your computer (compression, delegation, cost)
allowed-tools: Bash(python3:*), mcp__plugin_rtk_rtk-desktop__rtk_gain, mcp__remote-devices__plugin_rtk_rtk-desktop__rtk_gain
---
Call the rtk_gain tool (rtk plugin) once with no arguments. Its answer is Markdown: show it as is, not in a code block, and add no commentary. If the answer says the record of this task did not arrive, run `python3 "${CLAUDE_PLUGIN_ROOT}/hooks/gain.py"`, call rtk_gain again with ledger set to the JSON object printed between the ---ledger--- and --- markers, and show that answer instead.

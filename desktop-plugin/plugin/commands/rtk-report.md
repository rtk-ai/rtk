---
description: RTK test report for this task (pipeline state, model usage and cost, sub-agents, hook decisions)
allowed-tools: Bash(python3:*)
---
!`python3 "${CLAUDE_PLUGIN_ROOT}/hooks/pipeline.py" report`

Show the RTK report above exactly as written. It is Markdown: keep its heading, table and list as they are, do not wrap it in a code block, and add no commentary. If it is missing, run `python3 "${CLAUDE_PLUGIN_ROOT}/hooks/pipeline.py" report` in your own workspace and show its output the same way.

# RTK agent-output measurement

The repository now includes scripts/benchmark-agent-output.py for paired,
offline measurements. The report separates:

- raw_producer
- baseline_model_input
- candidate_model_input
- recovery_input
- hook_context
- tool_schema_context

The default counter is byte_estimate, defined as ceil(UTF-8 bytes / 4).
An optional tiktoken:<encoding> counter can be selected when the dependency
is deliberately installed. Neither counter is presented as a universal
billing-token count.

The current implementation has deterministic route and contract tests, but no
paid live-agent benchmark was run in this environment. A valid paired run must
record the base/candidate revisions, identical task inputs, producer exit
codes, complete rendered streams, recovery reads, and the selected counter.
Model routing or reasoning-effort changes must be reported separately from
RTK output compression.

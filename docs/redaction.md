# PII Redaction

RTK redacts PII and secrets from **all** command output before it reaches the
LLM — including `rtk proxy` raw passthrough. Redaction is **ON by default**.

## What gets redacted

| Category | Examples | Replacement |
|----------|----------|-------------|
| `email` | `ravi@example.com` | `[REDACTED:email]` |
| `phone` | `+91 9876543210`, `+1 415 555 0132`, `(022) 4000 1234` | `[REDACTED:phone]` |
| `pan` | Indian PAN `ABCDE1234F` | `[REDACTED:pan]` |
| `aadhaar` | 12-digit numbers that pass the **Verhoeff** checksum | `[REDACTED:aadhaar]` |
| `card` | 13–19 digit numbers that pass the **Luhn** checksum (spaces/dashes allowed) | `[REDACTED:card]` |
| `secrets` | AWS access keys (`AKIA…`), JWTs, `Bearer` tokens, PEM private-key blocks, `api_key=`/`password=`/`secret=` assignments | `[REDACTED:aws_key]`, `[REDACTED:jwt]`, `[REDACTED:token]`, `[REDACTED:private_key]`, `[REDACTED:secret]` |

Checksum gating (Luhn/Verhoeff) prevents false positives on commit SHAs,
epoch timestamps, UUIDs, job IDs and other long digit runs. The per-category
tag (rather than `****`) keeps output debuggable without leaking the value.

## Why this design is leak-proof

Redaction runs where raw process output is **first captured**
(`exec_capture`, `run_streaming`, `run_fallback`, pipe mode, and a
line-buffered copy loop in proxy mode) — not at print time. Filters, the
tee/recovery file, token tracking and the `never_worse` guard only ever see
already-redacted text, so no downstream "show raw instead" logic can
resurrect PII.

In proxy mode output is redacted per complete line, so a value split across
the 8 KiB pipe read boundary is still caught.

## Disabling / tuning

Per invocation:

```bash
rtk --no-redact proxy git log        # raw output for this run only
```

Persistently, in `~/.config/rtk/config.toml`
(macOS: `~/Library/Application Support/rtk/config.toml`):

```toml
[redaction]
enabled = true       # master switch (default: true)
email = true         # per-category toggles (default: true)
phone = true
pan = true
aadhaar = true
card = true
secrets = true

# lines matching these regexes are never redacted (e.g. test fixtures)
allowlist = ["EXAMPLE-DO-NOT-REDACT"]

# extra org-specific patterns
[[redaction.custom]]
name = "employee_id"
pattern = "EMP-\\d{6}"
```

Custom patterns are replaced with `[REDACTED:<name>]`. An invalid custom
regex is skipped with a warning — it never breaks the command.

## Known accepted gaps

- Commands with **no filter match** in the fallback path, and
  `RunMode::Passthrough` commands, inherit stdio directly (interactive
  tools, TTYs, `$EDITOR`) — RTK never sees their bytes, so it cannot redact
  them. Same class of bypass as documented for pre-redaction `rtk proxy`.
- In proxy mode, a single line longer than **1 MiB** with no newline is
  force-flushed in segments to bound memory; a value split exactly across
  that forced flush point can be missed (pathological output only).
- The `allowlist` is line-scoped; multi-line PEM blocks are redacted before
  the allowlist is applied.

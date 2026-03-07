# Review Comment Template

Use this template to generate GitHub PR review comments. Fill in each section based on the code-reviewer agent output. Comments are posted in **English** (international audience).

---

## Template

```markdown
## Review

**Scope**: Security, code quality, performance, test coverage, architecture

### Summary

{1–2 sentences: overall assessment. Be direct — what's the main takeaway?}

### Critical Issues 🔴

{List blocking issues that must be fixed before merge. For each:}
{- `file.rs:42` — Description of the problem. Why it matters. Suggested fix.}

{If none: "None found."}

### Important Issues 🟡

{List significant issues that should be fixed. For each:}
{- `file.rs:42` — Description. Why it matters. Suggested fix.}

{If none: "None found."}

### Suggestions 🟢

{List nice-to-haves and minor improvements. For each:}
{- Description. Context. Optional fix.}

{If none: omit this section.}

### What's Good ✅

{Always include at least 1 positive point. Be specific — what works well and why.}
{- Description of what's done right.}

---
*Automated review via [rtk](https://github.com/rtk-ai/rtk) `/pr-triage`*
```

---

## Formatting Rules

**Citation format** : `file.rs:42` or `` `code snippet` `` for inline references

**Issue severity** :
- 🔴 Critical : security vulnerability, data loss risk, broken functionality, test missing for new feature
- 🟡 Important : error handling gap, performance regression, scope creep, missing token savings assertion
- 🟢 Suggestion : naming, DRY opportunity, documentation, style

**RTK-specific checks to mention if relevant** :
- `lazy_static!` for regex (not inline `Regex::new()`)
- `anyhow::Result` + `.context("msg")` (no bare `?`, no `.unwrap()`)
- Fallback to raw command on filter failure
- Exit code propagation (`std::process::exit(code)`)
- Token savings assertion ≥60% in tests
- Real fixtures (not synthetic test data)
- No async/tokio dependencies (startup time)

**Tone** : Professional, constructive, factual. Challenge the code, not the person.
No superlatives ("great", "amazing", "perfect"). No filler ("as mentioned", "it's worth noting").

**Length** : Aim for 200–400 words. Long enough to be useful, short enough to be read.

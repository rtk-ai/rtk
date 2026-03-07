# Issue Comment Templates

Use these templates to generate GitHub issue comments. Select the appropriate template based on the recommended action from Phase 2. Comments are posted in **English** (international audience).

---

## Template 1 — Acknowledgment + Request Info

Use when: issue is valid but missing information to act on it (reproduction steps, version, environment, context).

```markdown
## Issue Triage

**Category**: {Bug | Feature | Enhancement | Question}
**Priority**: {P0 | P1 | P2 | P3}
**Effort estimate**: {XS | S | M | L | XL}

### Assessment

{1-2 sentences: what this issue is about and why it matters. Be direct.}

### Missing Information

To move forward, we need the following:

- {Specific missing info 1 — e.g., "RTK version (`rtk --version` output)"}
- {Specific missing info 2 — e.g., "Full command used and raw output"}
- {Specific missing info 3 — e.g., "OS and shell (macOS/Linux, zsh/bash)"}

### Next Steps

{What happens once the info is provided — e.g., "Once confirmed, we'll prioritize this for the next release."}

---
*Triaged via [rtk](https://github.com/rtk-ai/rtk) `/issue-triage`*
```

---

## Template 2 — Duplicate

Use when: this issue is a duplicate of an existing open (or recently closed) issue.

```markdown
## Duplicate Issue

This issue covers the same problem as #{original_number}: **{original_title}**.

### Overlap

{1-2 sentences explaining the overlap — what's identical or nearly identical between the two issues.}

If your situation differs in an important way (different command, different OS, different error message), please reopen and add that context. Otherwise, follow the original issue for updates.

---
*Triaged via [rtk](https://github.com/rtk-ai/rtk) `/issue-triage`*
```

---

## Template 3 — Close (Stale)

Use when: issue has had no activity for >90 days and there's been no engagement.

```markdown
## Closing: No Activity

This issue has been open for {N} days without activity. To keep the backlog actionable, we're closing it.

If this is still relevant:
- Reopen and add context about your current setup
- Or reference this issue in a new one if the problem has evolved

Thanks for taking the time to report it.

---
*Triaged via [rtk](https://github.com/rtk-ai/rtk) `/issue-triage`*
```

---

## Template 4 — Close (Out of Scope)

Use when: issue requests something that doesn't align with RTK's design goals (e.g., adding async runtime, platform-specific features outside scope, changing core behavior).

```markdown
## Closing: Out of Scope

After review, this request falls outside RTK's current design goals.

### Rationale

{1-2 sentences explaining why — be specific. Reference design constraints if relevant, e.g., "RTK is intentionally single-threaded with zero async dependencies to maintain <10ms startup time."}

### Alternatives

{If applicable: what the user can do instead. E.g., "For this use case, `rtk proxy <cmd>` gives you raw output while still tracking usage metrics."}

If the use case evolves or the scope changes in a future version, feel free to reopen with updated context.

---
*Triaged via [rtk](https://github.com/rtk-ai/rtk) `/issue-triage`*
```

---

## Formatting Rules

**Tone** : Professional, constructive, factual. Help the user move forward. Challenge the issue scope, not the person who filed it.

**Length** : 100-250 words per comment. Long enough to be useful, short enough to respect the reader's time.

**Specificity** : Always name the exact command, file, or behavior in question. Vague comments waste everyone's time.

**No superlatives** : Don't write "great issue", "excellent report", "amazing catch". Just address the substance.

**Priority labels** :
- P0 — Critical: security vulnerability, data loss, broken core functionality
- P1 — High: significant bug affecting common workflows, actionable this sprint
- P2 — Medium: valid issue, queue for backlog
- P3 — Low: nice-to-have, future consideration

**Effort labels** :
- XS : <1 hour
- S : 1-4 hours
- M : 1-2 days
- L : 3-5 days
- XL : >1 week

**RTK-specific context to include when relevant** :
- Mention `rtk --version` as the first diagnostic step for bug reports
- Reference the relevant module (`src/git.rs`, `src/vitest_cmd.rs`, etc.) when known
- Link to the filter development checklist in CLAUDE.md for feature requests that involve new commands
- Note performance constraints (<10ms startup) when rejecting async/heavy dependency requests

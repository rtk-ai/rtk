# Contributor FAQ

## Windows paths and discover/slug matching

Windows filesystems are usually **case-insensitive** but **case-preserving**. Normalize paths (lowercase + unified separators) before slug comparison.

## Grep: why `-E` may be stripped

RTK may forward to ripgrep, which does not accept GNU grep bare `-E`. Prefer patterns without relying on `-E`.

```bash
rtk grep 'foo|bar' path/
```

## `rtk init` frontmatter fields

| Field | Purpose |
| --- | --- |
| `name` | Display name |
| `description` | One-line summary |
| `always_on` / `trigger` | Activation |
| `globs` | Path filters |
| `tools` | Allowed tools |

```yaml
---
name: review-helper
description: Structured review notes
always_on: false
globs: ["**/*.{ts,tsx,py}"]
---
```

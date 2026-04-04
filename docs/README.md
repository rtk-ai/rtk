# RTK Documentation — Interface Contract

This directory is the source of truth for user-facing and contributor documentation.
It feeds the RTK website via the `prepare-docs.mjs` pipeline in `rtk-ai/rtk-website`.

## Structure

```
docs/
  README.md            <- This file (interface contract — do not remove)
  guide/               -> User-facing documentation (Tab "Guide")
  reference/           -> Contributor/technical documentation (Tab "Reference")
  architecture/        -> Conceptual/visual documentation (Tab "Architecture")
```

## Frontmatter (required on every .md)

Every markdown file under `docs/guide/`, `docs/reference/`, and `docs/architecture/`
must include this frontmatter block at the top:

```yaml
---
title: string          # Page title (used in sidebar + search)
description: string    # One-line summary for search results and SEO
sidebar:
  order: number        # Position within the sidebar group (1 = first)
---
```

The `prepare-docs.mjs` pipeline validates this contract at build time and **fails fast**
with a clear error message if frontmatter is missing or malformed.

## Conventions

- **Filenames**: kebab-case, `.md` only (e.g., `getting-started.md`, `quick-start.md`)
- **Subdirectories**: become sidebar groups in Starlight (directory name = group label)
- **Internal links**: relative within the same tab (`./foo.md`, `./getting-started/installation.md`)
- **Cross-tab links**: full path from `docs/` root (`../../reference/internals/command-routing.md`)
- **Diagrams**: Mermaid in fenced code blocks (` ```mermaid `)
- **Code samples**: always specify the language (`rust`, `toml`, `bash`, `shell`)
- **Language**: English only

## Tabs overview

| Tab | Path | Audience |
|-----|------|----------|
| Guide | `docs/guide/` | End users installing and using RTK |
| Reference | `docs/reference/` | Contributors, maintainers, integrators |
| Architecture | `docs/architecture/` | Readers exploring design decisions and diagrams |

## Root files (do not move or modify structure)

The following files live at the repository root and are **not** managed by this pipeline.
They are the canonical source for GitHub display and remain unchanged.

- `README.md` — Project overview
- `INSTALL.md` — Installation reference (full)
- `CONTRIBUTING.md` — Contribution guide
- `SECURITY.md` — Security policy
- `ARCHITECTURE.md` — Full architecture document
- `CHANGELOG.md` — Release history

The guide files in `docs/` are derived, English-only, structured versions intended
for the website. They reference root files as source material but do not replace them.

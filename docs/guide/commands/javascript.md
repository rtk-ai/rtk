---
title: JavaScript / TypeScript
description: RTK filters for pnpm, npm, vitest, tsc, ESLint, Next.js, Playwright, and Prisma
sidebar:
  order: 4
---

# JavaScript / TypeScript

RTK covers the full JS/TS toolchain: package managers, test runners, type checking, linting, build tools, and ORM.

## vitest run — 99.5% savings

```bash
rtk vitest run [args...]
```

Shows failures only. One of the highest savings rates in RTK — vitest verbose output can be massive.

## Playwright — 94% savings

```bash
rtk playwright [args...]
```

Shows failures and summaries only, strips progress output.

## tsc — 83% savings

```bash
rtk tsc [args...]
```

Groups TypeScript errors by file and error code.

**Before (50 lines for 4 errors):**
```
src/api.ts(12,5): error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.
src/api.ts(15,10): error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.
src/api.ts(20,3): error TS7006: Parameter 'x' implicitly has an 'any' type.
src/utils.ts(5,1): error TS2304: Cannot find name 'foo'.
```

**After (15 lines):**
```
src/api.ts (3 errors)
  TS2345: Argument type mismatch (x2)
  TS7006: Parameter implicitly has 'any'
src/utils.ts (1 error)
  TS2304: Cannot find name 'foo'
```

## ESLint / Biome — 84% savings

```bash
rtk lint [args...]
rtk lint biome [args...]
```

Groups violations by rule and file. Auto-detects the linter.

## prettier

```bash
rtk prettier --check .    # 70% savings
rtk prettier --write src/
```

Shows only files that need formatting.

## Next.js build — 87% savings

```bash
rtk next [args...]
rtk pnpm build    # delegates to Next.js filter
```

Compact output with route metrics.

## pnpm

| Command | Description | Savings |
|---------|-------------|---------|
| `rtk pnpm list [-d N]` | Compact dependency tree | ~70% |
| `rtk pnpm outdated` | Outdated packages: `pkg: old -> new` | ~80% |
| `rtk pnpm install [pkgs...]` | Filters progress bars | ~60% |
| `rtk pnpm build` | Delegates to Next.js filter | ~87% |
| `rtk pnpm typecheck` | Delegates to tsc filter | ~83% |

Unrecognized subcommands pass through to pnpm directly.

## npm

```bash
rtk npm [args...]    # e.g. rtk npm run build
```

Filters npm boilerplate (progress bars, headers, audit notices).

## npx — smart routing

```bash
rtk npx [args...]
```

Routes automatically to specialized filters:

- `rtk npx tsc` → tsc filter
- `rtk npx eslint` → lint filter
- `rtk npx prisma` → prisma filter
- Other → passthrough filter

## Prisma

| Command | Description |
|---------|-------------|
| `rtk prisma generate` | Client generation (removes ASCII art) |
| `rtk prisma migrate dev [--name N]` | Create and apply a migration |
| `rtk prisma migrate status` | Migration status |
| `rtk prisma migrate deploy` | Deploy to production |
| `rtk prisma db-push` | Push schema |

## Universal format detector

```bash
rtk format [args...]
```

Auto-detects the project formatter (prettier, black, ruff format, rustfmt) and applies a unified compact filter.

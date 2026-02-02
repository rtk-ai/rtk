# Changelog

All notable changes to rtk (Rust Token Killer) will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.8.0](https://github.com/rtk-ai/rtk/compare/v0.7.1...v0.8.0) (2026-02-02)


### Features

* add comprehensive security review workflow for PRs ([1ca6e81](https://github.com/rtk-ai/rtk/commit/1ca6e81bdf16a7eab503d52b342846c3519d89ff))
* add comprehensive security review workflow for PRs ([66101eb](https://github.com/rtk-ai/rtk/commit/66101ebb65076359a1530d8f19e11a17c268bce2))

## [0.7.1](https://github.com/pszymkowiak/rtk/compare/v0.7.0...v0.7.1) (2026-02-02)


### Features

* **execution time tracking**: Add command execution time metrics to `rtk gain` analytics
  - Total execution time and average time per command displayed in summary
  - Time column in "By Command" breakdown showing average execution duration
  - Daily breakdown (`--daily`) includes time metrics per day
  - JSON export includes `total_time_ms` and `avg_time_ms` fields
  - CSV export includes execution time columns
  - Backward compatible: historical data shows 0ms (pre-tracking)
  - Negligible overhead: <0.1ms per command
  - New SQLite column: `exec_time_ms` in commands table
* **parser infrastructure**: Three-tier fallback system for robust output parsing
  - Tier 1: Full JSON parsing with complete structured data
  - Tier 2: Degraded parsing with regex fallback and warnings
  - Tier 3: Passthrough with truncated raw output and error markers
  - Guarantees RTK never returns false data silently
* **migrate commands to OutputParser**: vitest, playwright, pnpm now use robust parsing
  - JSON parsing with safe fallbacks for all modern JS tooling
  - Improved error handling and debugging visibility
* **local LLM analysis**: Add economics analysis and comprehensive test scripts
  - `scripts/rtk-economics.sh` for token savings ROI analysis
  - `scripts/test-all.sh` with 69 assertions covering all commands
  - `scripts/test-aristote.sh` for T3 Stack project validation


### Bug Fixes

* convert rtk ls from reimplementation to native proxy for better reliability
* trigger release build after release-please creates tag


### Documentation

* add execution time tracking test guide (TEST_EXEC_TIME.md)
* comprehensive parser infrastructure documentation (src/parser/README.md)

## [0.7.0](https://github.com/pszymkowiak/rtk/compare/v0.6.0...v0.7.0) (2026-02-01)


### Features

* add discover command, auto-rewrite hook, and git show support ([ff1c759](https://github.com/pszymkowiak/rtk/commit/ff1c7598c240ca69ab51f507fe45d99d339152a0))
* discover command, auto-rewrite hook, git show ([c9c64cf](https://github.com/pszymkowiak/rtk/commit/c9c64cfd30e2c867ce1df4be508415635d20132d))


### Bug Fixes

* forward args in rtk git push/pull to support -u, remote, branch ([4bb0130](https://github.com/pszymkowiak/rtk/commit/4bb0130695ad2f5d91123afac2e3303e510b240c))

## [0.6.0](https://github.com/pszymkowiak/rtk/compare/v0.5.2...v0.6.0) (2026-02-01)


### Features

* cargo build/test/clippy with compact output ([bfd5646](https://github.com/pszymkowiak/rtk/commit/bfd5646f4eac32b46dbec05f923352a3e50c19ef))
* curl with auto-JSON detection ([314accb](https://github.com/pszymkowiak/rtk/commit/314accbfd9ac82cc050155c6c47dfb76acab14ce))
* gh pr create/merge/diff/comment/edit + gh api ([517a93d](https://github.com/pszymkowiak/rtk/commit/517a93d0e4497414efe7486410c72afdad5f8a26))
* git branch, fetch, stash, worktree commands ([bc31da8](https://github.com/pszymkowiak/rtk/commit/bc31da8ad9d9e91eee8af8020e5bd7008da95dd2))
* npm/npx routing, pnpm build/typecheck, --skip-env flag ([49b3cf2](https://github.com/pszymkowiak/rtk/commit/49b3cf293d856ff3001c46cff8fee9de9ef501c5))
* shared infrastructure for new commands ([6c60888](https://github.com/pszymkowiak/rtk/commit/6c608880e9ecbb2b3569f875e7fad37d1184d751))
* shared infrastructure for new commands ([9dbc117](https://github.com/pszymkowiak/rtk/commit/9dbc1178e7f7fab8a0695b624ed3744ab1a8bf02))

## [0.5.2](https://github.com/pszymkowiak/rtk/compare/v0.5.1...v0.5.2) (2026-01-30)


### Bug Fixes

* release pipeline trigger and version-agnostic package URLs ([108d0b5](https://github.com/pszymkowiak/rtk/commit/108d0b5ea316ab33c6998fb57b2caf8c65ebe3ef))
* release pipeline trigger and version-agnostic package URLs ([264539c](https://github.com/pszymkowiak/rtk/commit/264539cf20a29de0d9a1a39029c04cb8eb1b8f10))

## [0.5.1](https://github.com/pszymkowiak/rtk/compare/v0.5.0...v0.5.1) (2026-01-30)


### Bug Fixes

* 3 issues (latest tag, ccusage fallback, versioning) ([d773ec3](https://github.com/pszymkowiak/rtk/commit/d773ec3ea515441e6c62bbac829f45660cfaccde))
* patrick's 3 issues (latest tag, ccusage fallback, versioning) ([9e322e2](https://github.com/pszymkowiak/rtk/commit/9e322e2aee9f7239cf04ce1bf9971920035ac4bb))

## [0.5.0](https://github.com/pszymkowiak/rtk/compare/v0.4.0...v0.5.0) (2026-01-30)


### Features

* add comprehensive claude code economics analysis ([ec1cf9a](https://github.com/pszymkowiak/rtk/commit/ec1cf9a56dd52565516823f55f99a205cfc04558))
* comprehensive economics analysis and code quality improvements ([8e72e7a](https://github.com/pszymkowiak/rtk/commit/8e72e7a8b8ac7e94e9b13958d8b6b8e9bf630660))


### Bug Fixes

* comprehensive code quality improvements ([5b840cc](https://github.com/pszymkowiak/rtk/commit/5b840cca492ea32488d8c80fd50d3802a0c41c72))
* optimize HashMap merge and add safety checks ([3b847f8](https://github.com/pszymkowiak/rtk/commit/3b847f863a90b2e9a9b7eb570f700a376bce8b22))

## [0.4.0](https://github.com/pszymkowiak/rtk/compare/v0.3.1...v0.4.0) (2026-01-30)


### Features

* add comprehensive temporal audit system for token savings analytics ([76703ca](https://github.com/pszymkowiak/rtk/commit/76703ca3f5d73d3345c2ed26e4de86e6df815aff))
* Comprehensive Temporal Audit System for Token Savings Analytics ([862047e](https://github.com/pszymkowiak/rtk/commit/862047e387e95b137973983b4ebad810fe5b4431))

## [0.3.1](https://github.com/pszymkowiak/rtk/compare/v0.3.0...v0.3.1) (2026-01-29)


### Bug Fixes

* improve command robustness and flag support ([c2cd691](https://github.com/pszymkowiak/rtk/commit/c2cd691c823c8b1dd20d50d01486664f7fd7bd28))
* improve command robustness and flag support ([d7d8c65](https://github.com/pszymkowiak/rtk/commit/d7d8c65b86d44792e30ce3d0aff9d90af0dd49ed))

## [0.3.0](https://github.com/pszymkowiak/rtk/compare/v0.2.1...v0.3.0) (2026-01-29)


### Features

* add --quota flag to rtk gain with tier-based analysis ([26b314d](https://github.com/pszymkowiak/rtk/commit/26b314d45b8b0a0c5c39fb0c17001ecbde9d97aa))
* add CI/CD automation (release management and automated metrics) ([22c3017](https://github.com/pszymkowiak/rtk/commit/22c3017ed5d20e5fb6531cfd7aea5e12257e3da9))
* add GitHub CLI integration (depends on [#9](https://github.com/pszymkowiak/rtk/issues/9)) ([341c485](https://github.com/pszymkowiak/rtk/commit/341c48520792f81889543a5dc72e572976856bbb))
* add GitHub CLI integration with token optimizations ([0f7418e](https://github.com/pszymkowiak/rtk/commit/0f7418e958b23154cb9dcf52089a64013a666972))
* add modern JavaScript tooling support ([b82fa85](https://github.com/pszymkowiak/rtk/commit/b82fa85ae5fe0cc1f17d8acab8c6873f436a4d62))
* add modern JavaScript tooling support (lint, tsc, next, prettier, playwright, prisma) ([88c0174](https://github.com/pszymkowiak/rtk/commit/88c0174d32e0603f6c5dcc7f969fa8f988573ec6))
* add Modern JS Stack commands to benchmark script ([b868987](https://github.com/pszymkowiak/rtk/commit/b868987f6f48876bb2ce9a11c9cad12725401916))
* add quota analysis with multi-tier support ([64c0b03](https://github.com/pszymkowiak/rtk/commit/64c0b03d4e4e75a7051eac95be2d562797f1a48a))
* add shared utils module for JS stack commands ([0fc06f9](https://github.com/pszymkowiak/rtk/commit/0fc06f95098e00addf06fe71665638ab2beb1aac))
* CI/CD automation (versioning, benchmarks, README auto-update) ([b8bbfb8](https://github.com/pszymkowiak/rtk/commit/b8bbfb87b4dc2b664f64ee3b0231e346a2244055))


### Bug Fixes

* **ci:** correct rust-toolchain action name ([9526471](https://github.com/pszymkowiak/rtk/commit/9526471530b7d272f32aca38ace7548fd221547e))

## [Unreleased]

### Added
- `prettier` command for format checking with package manager auto-detection (pnpm/yarn/npx)
  - Shows only files needing formatting (~70% token reduction)
  - Exit code preservation for CI/CD compatibility
- `playwright` command for E2E test output filtering (~94% token reduction)
  - Shows only test failures and slow tests
  - Summary with pass/fail counts and timing
- `lint` command with ESLint/Biome support and pnpm detection
  - Groups violations by rule and file (~84% token reduction)
  - Shows top violators for quick navigation
- `tsc` command for TypeScript compiler output filtering
  - Groups errors by file and error code (~83% token reduction)
  - Shows top 10 affected files
- `next` command for Next.js build/dev output filtering (87% token reduction)
  - Extracts route count and bundle sizes
  - Highlights warnings and oversized bundles
- `prisma` command for Prisma CLI output filtering
  - Removes ASCII art and verbose logs (~88% token reduction)
  - Supports generate, migrate (dev/status/deploy), and db push
- `utils` module with common utilities (truncate, strip_ansi, execute_command)
  - Shared functionality for consistent output formatting
  - ANSI escape code stripping for clean parsing

### Changed
- Refactored duplicated code patterns into `utils.rs` module
- Improved package manager detection across all modern JS commands

## [0.2.1] - 2026-01-29

See upstream: https://github.com/pszymkowiak/rtk

## Links

- **Repository**: https://github.com/FlorianBruniaux/rtk (fork)
- **Upstream**: https://github.com/pszymkowiak/rtk
- **Issues**: https://github.com/FlorianBruniaux/rtk/issues

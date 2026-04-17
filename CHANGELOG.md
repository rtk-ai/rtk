# Changelog

All notable changes to rtk (Rust Token Killer) will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.37.0](https://github.com/rtk-ai/rtk/compare/v0.36.0...v0.37.0) (2026-04-17)


### Features

* **discover:** handle more npm/npx/pnpm/pnpx patterns ([9e96caa](https://github.com/rtk-ai/rtk/commit/9e96caa0a18a95c84da82ba57716a9d3ef86d0c8))
* **refacto-core:** binary hook w/ native cmd exec + streaming ([e7b7f9a](https://github.com/rtk-ai/rtk/commit/e7b7f9ab665a0f7303d41d23ad156d24e5e8964e))


### Bug Fixes

* **docs:** use release please changelog no manual ([7591a14](https://github.com/rtk-ai/rtk/commit/7591a14e4ceb732ab7ca160ac01a852926abe77a))
* isolate cursor hook tests from local settings (determinist) ([d8ddefe](https://github.com/rtk-ai/rtk/commit/d8ddefe78efe25c35bb2a2f9083f2eacb9dd7274))
* P0+P1 fixes from pre-merge review of hook engine ([df8e035](https://github.com/rtk-ai/rtk/commit/df8e03558d4d6cc2f5cbac91c63ab1b3b51d3bcd))
* P0+P1 fixes from pre-merge review of hook engine ([d34389c](https://github.com/rtk-ai/rtk/commit/d34389c3d0936c2b0790e14f450bb50a28a7edf7))
* rename ship.md to ship/SKILL.md to match develop ([5916ecd](https://github.com/rtk-ai/rtk/commit/5916ecd86fb319c2519a0b4fb2891309833a3bb4))
* **runner:** preserve fd separation on command failure ([e92d099](https://github.com/rtk-ai/rtk/commit/e92d0993c93f0b732316dfa932d265aeca7488d6))
* **stream:** missing stderr fields ([a1d46f3](https://github.com/rtk-ai/rtk/commit/a1d46f39c291e3356b9c26a062bde05ba1de591a))

## [0.36.0](https://github.com/rtk-ai/rtk/compare/v0.35.0...v0.36.0) (2026-04-13)


### Features

* **benchmark:** add multipass VM integration test suite ([6e7863b](https://github.com/rtk-ai/rtk/commit/6e7863bf313b0d18a47cf0ca2cdaea03cc2ed900))
* **benchmark:** add multipass VM integration test suite ([d22759b](https://github.com/rtk-ai/rtk/commit/d22759b8c5254ad9c4a455f10cb7de75e92df581))
* **benchmark:** add Swift ecosystem tests (6 commands + savings) ([1fbb6d9](https://github.com/rtk-ai/rtk/commit/1fbb6d935b4a0d031a7862cba312eebe1303ba9b))
* **init:** add native support for Kilo Code and Google Antigravity ([d0a3797](https://github.com/rtk-ai/rtk/commit/d0a3797ec580f96948489d1e7c3329ac22a6c4eb))
* **init:** add support for kilocode and antigravity agents ([66b90f1](https://github.com/rtk-ai/rtk/commit/66b90f1ed3de81acdce61164c068c24ed7ef29db))
* **pnpm:** Add filter argument support ([2ba8d37](https://github.com/rtk-ai/rtk/commit/2ba8d372df186b4056a3b8906fc25cde8586dd42))
* **skills:** add /pr-review skill for batch PR review ([21e67a1](https://github.com/rtk-ai/rtk/commit/21e67a1113041b74542d0285e5f74587dfb30b65))
* **telemetry:** enrich daily ping with gap detection and quality metrics ([644c50f](https://github.com/rtk-ai/rtk/commit/644c50f786e5c567617e7ea907c5f312797b1265))


### Bug Fixes

* **benchmark:** address PR review feedback ([87ee81f](https://github.com/rtk-ai/rtk/commit/87ee81f08be5e7b1ca79513b1a91925d455f4f5c))
* **benchmark:** address review feedback from @FlorianBruniaux ([d13c185](https://github.com/rtk-ai/rtk/commit/d13c185aac64d14288b574df44623723a69e7b95))
* **ccusage:** add --yes flag and warn when falling back to npx ([f68fa00](https://github.com/rtk-ai/rtk/commit/f68fa0087c03d6882993b7b3eaee98e1dbab41b4))
* **clippy:** show full error blocks instead of truncated headline ([95d9d13](https://github.com/rtk-ai/rtk/commit/95d9d134b0b76d83b6162614b0a79269b2135f40))
* **clippy:** show full error blocks instead of truncated headline ([f4074f8](https://github.com/rtk-ai/rtk/commit/f4074f898a9b73b72bbcd8b18afab4831dcda406)), closes [#602](https://github.com/rtk-ai/rtk/issues/602)
* **curl:** skip JSON schema conversion for internal/localhost URLs ([577c311](https://github.com/rtk-ai/rtk/commit/577c311ecaaa8ae94f22dbe252152424d4333d04))
* **discover:** preserve golangci-lint flags in rewrite ([d85303e](https://github.com/rtk-ai/rtk/commit/d85303ec4893deb904260f5dc11b7df906a50c07))
* **docs:** update TELEMETRY.md to match code after review fixes ([be5c057](https://github.com/rtk-ai/rtk/commit/be5c0576d95566f37f266fd9f92e2a1b263697bd))
* **find:** include hidden files when pattern targets dotfiles ([#1101](https://github.com/rtk-ai/rtk/issues/1101)) ([dbeeaed](https://github.com/rtk-ai/rtk/commit/dbeeaed16aee79674ec2fd3778b7b11b10b847c6))
* **git:** re-insert -- separator when clap consumes it from git diff args ([#1215](https://github.com/rtk-ai/rtk/issues/1215)) ([9979c69](https://github.com/rtk-ai/rtk/commit/9979c699307a4adad2c2df0f2bc3b663df653311))
* **git:** remove -u short alias from --ultra-compact to fix git push -u ([6b76fdb](https://github.com/rtk-ai/rtk/commit/6b76fdb87d7c54cfc2a1b0e6117dd78b8430910b))
* **golangci-lint:** restore run wrapper and align guidance ([4f4e4d2](https://github.com/rtk-ai/rtk/commit/4f4e4d2b5a3529030fe4089f60d2f4b8740b5d53))
* **golangci-lint:** support inline global flags before run ([24f2ada](https://github.com/rtk-ai/rtk/commit/24f2adaf8fb541c2564fa7dfb423947932e68fb4))
* **go:** prevent double-counted failures when test-level fail also triggers package-level fail ([#958](https://github.com/rtk-ai/rtk/issues/958)) ([4fc15ef](https://github.com/rtk-ai/rtk/commit/4fc15ef2c1c80336ffaafa4179db4cee6f39236a))
* **go:** prevent double-counting failures when package-level fail cascades from test failures ([#958](https://github.com/rtk-ai/rtk/issues/958)) ([9722d5e](https://github.com/rtk-ai/rtk/commit/9722d5ebd8916f9b398bdc01b1102d42ab2b8795))
* **hooks:** ensure default permission verdict prompts user for confirmation ([40462c0](https://github.com/rtk-ai/rtk/commit/40462c05e66f116928de365a0d271bdfd61cec72))
* **hooks:** require all segments to match allow rules ([#1213](https://github.com/rtk-ai/rtk/issues/1213)) ([40c9dbc](https://github.com/rtk-ai/rtk/commit/40c9dbc7dbbf9332d6859060765c582a880f0fde))
* **init:** honor CODEX_HOME for Codex global paths ([d442799](https://github.com/rtk-ai/rtk/commit/d442799e34d522c87a6eb60c2ff373385d201315))
* **init:** install Codex global instructions in CODEX_HOME ([a257688](https://github.com/rtk-ai/rtk/commit/a2576883a27c5f915ba0ae7883a51006411b3ae5))
* **json:** rename --schema to --keys-only, closes [#621](https://github.com/rtk-ai/rtk/issues/621) ([c16713a](https://github.com/rtk-ai/rtk/commit/c16713a973b563a6cba283c830b67c8c470e419f))
* **ls:** filter quality wrong truncation ([aa6317f](https://github.com/rtk-ai/rtk/commit/aa6317fb83a5d9883623a4d3bee7a25bc99dcb4c))
* **permissions:** glob_matches middle-wildcard matches commands without trailing args ([#1105](https://github.com/rtk-ai/rtk/issues/1105)) ([3db8070](https://github.com/rtk-ai/rtk/commit/3db8070b51b9a312fcca20a8460d3d6259cc38b7))
* **pnpm:** list command not working ([ba235d8](https://github.com/rtk-ai/rtk/commit/ba235d85974c0a85b25e290a8bb83648800438a6))
* **pytest:** -q mode summary line not detected ([57502a5](https://github.com/rtk-ai/rtk/commit/57502a5bef1fb56109a57cf2ea7377fd271253a7))
* report package-level failures (timeouts, signals) in go test summary ([0b1c32b](https://github.com/rtk-ai/rtk/commit/0b1c32b3cc9a3e73418d401d1d481c1611c7ec0b))
* report package-level failures (timeouts, signals) in go test summary ([c85a387](https://github.com/rtk-ai/rtk/commit/c85a387363e2079234b6141aad26418172c0e61a)), closes [#958](https://github.com/rtk-ai/rtk/issues/958)
* **security:** correct email domain from .dev to .app ([47383e8](https://github.com/rtk-ai/rtk/commit/47383e80197fc56e38f880f33a6b54261b82523c))
* **tee:** prevent panic on UTF-8 multi-byte truncation boundary ([da486bf](https://github.com/rtk-ai/rtk/commit/da486bf394330c804cd1cd12e4b6835f18de5205))
* **telemetry:** 7 bugs in enrichment — privacy leak, broken meta_usage, pricing ([15f666d](https://github.com/rtk-ai/rtk/commit/15f666dd8dbd18648cb7bd14a6f9f3cac2f7d10b))
* **telemetry:** clean code ([8156081](https://github.com/rtk-ai/rtk/commit/81560812610686fa5ca3633c2bf0b79c05eaa7d9))
* **telemetry:** consent, erasure, auth, docs ([2e4cc4b](https://github.com/rtk-ai/rtk/commit/2e4cc4bb5226444c8c0bfc827baf0c101c3759e8))
* **telemetry:** non-terminal consent, single config load ([7821e98](https://github.com/rtk-ai/rtk/commit/7821e9872fd1f1ae9b40eb8a4458049869acc36b))
* **telemetry:** RGPD-compliant, consent gate, erasure, privacy controls ([6a5bc84](https://github.com/rtk-ai/rtk/commit/6a5bc847e06cf6066e6f4aeed5a3ad0803a3649b))

## [0.35.0](https://github.com/rtk-ai/rtk/compare/v0.34.3...v0.35.0) (2026-04-06)


### Features

* **aws:** expand CLI filters from 8 to 25 subcommands ([402c48e](https://github.com/rtk-ai/rtk/commit/402c48e66988e638a5b4f4dd193238fc1d0fe18f))


### Bug Fixes

* **cmd:** read/cat multiple file and consistent behavior ([3f58018](https://github.com/rtk-ai/rtk/commit/3f58018f4af1d7206457929cf80bb4534203c3ee))
* **docs:** clean some docs + disclaimer ([deda44f](https://github.com/rtk-ai/rtk/commit/deda44f73607981f3d27ecc6341ce927aab34d37))
* **gh:** pass through gh pr merge instead of canned response ([#938](https://github.com/rtk-ai/rtk/issues/938)) ([8465ca9](https://github.com/rtk-ai/rtk/commit/8465ca953fa9d70dcc971a941c19465d456eb7d4))
* **gh:** pass through gh pr merge instead of canned response ([#938](https://github.com/rtk-ai/rtk/issues/938)) ([e1f2845](https://github.com/rtk-ai/rtk/commit/e1f2845df06a8d8b8325945dc4940ec5f530e4cc))
* **git:** inherit stdin for commit and push to preserve SSH signing ([#733](https://github.com/rtk-ai/rtk/issues/733)) ([eefeae4](https://github.com/rtk-ai/rtk/commit/eefeae45656ff2607c3f519c8eae235e3f0fe411))
* **git:** inherit stdin for commit and push to preserve SSH signing ([#733](https://github.com/rtk-ai/rtk/issues/733)) ([6cee6c6](https://github.com/rtk-ai/rtk/commit/6cee6c60b80f914ed9505e3925d85cadec43ab97))
* **git:** preserve full diff hunk headers ([62f4452](https://github.com/rtk-ai/rtk/commit/62f445227679f3df293fe35e9b18cc5ab39d7963))
* **git:** preserve full diff hunk headers ([09b3ff9](https://github.com/rtk-ai/rtk/commit/09b3ff9424e055f5fe25e535e5b60e077f8344f9))
* **go:** avoid false build errors from download logs ([9c1cf2f](https://github.com/rtk-ai/rtk/commit/9c1cf2f403534fa7874638b1b983c2d7f918a185))
* **go:** avoid false build errors from download logs ([d44fd3e](https://github.com/rtk-ai/rtk/commit/d44fd3e034208e3bcd59c2c46f7720eec4f10c98))
* **go:** cover more build failure shapes ([2425ad6](https://github.com/rtk-ai/rtk/commit/2425ad68e5386d19e5ec9ff1ca151a6d2c9a56d3))
* **go:** preserve failing test location context ([1481bc5](https://github.com/rtk-ai/rtk/commit/1481bc590924031456a6022510275c29c09e330e))
* **go:** preserve failing test location context ([374fe64](https://github.com/rtk-ai/rtk/commit/374fe64cfbedcd676733973e81a63a6dfecbb1b7))
* **go:** restore build error coverage ([1177c9c](https://github.com/rtk-ai/rtk/commit/1177c9c873ac63b6c0bcc9e1b664a705baa0ad7a))
* **grep:** close subprocess stdin to prevent memory leak ([#897](https://github.com/rtk-ai/rtk/issues/897)) ([7217562](https://github.com/rtk-ai/rtk/commit/72175623551f40b581b4a7f6ed966c1e4a9c7358))
* **grep:** close subprocess stdin to prevent memory leak ([#897](https://github.com/rtk-ai/rtk/issues/897)) ([09979cf](https://github.com/rtk-ai/rtk/commit/09979cf29701a1b775bcac761d24ec0e055d1bec))
* **hook_check:** detect missing integrations ([9cf9ccc](https://github.com/rtk-ai/rtk/commit/9cf9ccc1ac39f8bba37e932c7d318a3aa7a34ae9))
* **init:** remove opt-out instruction from telemetry message ([7571c8e](https://github.com/rtk-ai/rtk/commit/7571c8e101c41ee64c51e2bd64697f85f9142423))
* **init:** remove telemetry info lines from init output ([7dbef2c](https://github.com/rtk-ai/rtk/commit/7dbef2ce00824d26f2057e4c3c76e429e2e23088))
* **main:** kill zombie processes + path for rtk md ([d16fc6d](https://github.com/rtk-ai/rtk/commit/d16fc6dacbfec912c21522939b15b7bbd9719487))
* **main:** kill zombie processes + path for rtk md + missing intergrations ([a919335](https://github.com/rtk-ai/rtk/commit/a919335519ed4a5259a212e56407cb312aa99bac))
* **merge:** changelog conflicts ([d92c5d2](https://github.com/rtk-ai/rtk/commit/d92c5d264a49483c8d6079e04d946a79bc990a74))
* **proxy:** kill child process on SIGINT/SIGTERM to prevent orphans ([d813919](https://github.com/rtk-ai/rtk/commit/d813919a24546e044e7844fc7ed05fef4ec24033))
* **proxy:** kill child process on SIGINT/SIGTERM to prevent orphans ([3318510](https://github.com/rtk-ai/rtk/commit/33185101fc122d0c11a25a4e02ac9f3a7dc7e3bb))
* **review:** address ChildGuard disarm, stdin dedup, hook masking ([d85fe33](https://github.com/rtk-ai/rtk/commit/d85fe3384b87c16fafd25ec7bcadbff6e69f3f1f))
* **security:** default to ask when no permission rule matches ([#886](https://github.com/rtk-ai/rtk/issues/886)) ([158c745](https://github.com/rtk-ai/rtk/commit/158c74527f6591d372e40a78cd604d73a20649a9))
* **security:** default to ask when no permission rule matches ([#886](https://github.com/rtk-ai/rtk/issues/886)) ([41a6c6b](https://github.com/rtk-ai/rtk/commit/41a6c6bf6da78a4754794fdc6a1469df2e327920))
* **tracking:** use std::env::temp_dir() for compatibility (instead of unix tmp) ([e918661](https://github.com/rtk-ai/rtk/commit/e918661440d7b50321f0535032f52c5e87aaf3cb))

## [Unreleased]

### Bug Fixes

* **git:** remove `-u` short alias from `--ultra-compact` to fix `git push -u` upstream tracking ([#1086](https://github.com/rtk-ai/rtk/issues/1086))

## [0.35.0](https://github.com/rtk-ai/rtk/compare/v0.34.3...v0.35.0) (2026-04-06)


### Features

* **aws:** expand CLI filters from 8 to 25 subcommands ([402c48e](https://github.com/rtk-ai/rtk/commit/402c48e66988e638a5b4f4dd193238fc1d0fe18f))


### Bug Fixes

* **cmd:** read/cat multiple file and consistent behavior ([3f58018](https://github.com/rtk-ai/rtk/commit/3f58018f4af1d7206457929cf80bb4534203c3ee))
* **docs:** clean some docs + disclaimer ([deda44f](https://github.com/rtk-ai/rtk/commit/deda44f73607981f3d27ecc6341ce927aab34d37))
* **gh:** pass through gh pr merge instead of canned response ([#938](https://github.com/rtk-ai/rtk/issues/938)) ([8465ca9](https://github.com/rtk-ai/rtk/commit/8465ca953fa9d70dcc971a941c19465d456eb7d4))
* **gh:** pass through gh pr merge instead of canned response ([#938](https://github.com/rtk-ai/rtk/issues/938)) ([e1f2845](https://github.com/rtk-ai/rtk/commit/e1f2845df06a8d8b8325945dc4940ec5f530e4cc))
* **git:** inherit stdin for commit and push to preserve SSH signing ([#733](https://github.com/rtk-ai/rtk/issues/733)) ([eefeae4](https://github.com/rtk-ai/rtk/commit/eefeae45656ff2607c3f519c8eae235e3f0fe411))
* **git:** inherit stdin for commit and push to preserve SSH signing ([#733](https://github.com/rtk-ai/rtk/issues/733)) ([6cee6c6](https://github.com/rtk-ai/rtk/commit/6cee6c60b80f914ed9505e3925d85cadec43ab97))
* **git:** preserve full diff hunk headers ([62f4452](https://github.com/rtk-ai/rtk/commit/62f445227679f3df293fe35e9b18cc5ab39d7963))
* **git:** preserve full diff hunk headers ([09b3ff9](https://github.com/rtk-ai/rtk/commit/09b3ff9424e055f5fe25e535e5b60e077f8344f9))
* **go:** avoid false build errors from download logs ([9c1cf2f](https://github.com/rtk-ai/rtk/commit/9c1cf2f403534fa7874638b1b983c2d7f918a185))
* **go:** avoid false build errors from download logs ([d44fd3e](https://github.com/rtk-ai/rtk/commit/d44fd3e034208e3bcd59c2c46f7720eec4f10c98))
* **go:** cover more build failure shapes ([2425ad6](https://github.com/rtk-ai/rtk/commit/2425ad68e5386d19e5ec9ff1ca151a6d2c9a56d3))
* **go:** preserve failing test location context ([1481bc5](https://github.com/rtk-ai/rtk/commit/1481bc590924031456a6022510275c29c09e330e))
* **go:** preserve failing test location context ([374fe64](https://github.com/rtk-ai/rtk/commit/374fe64cfbedcd676733973e81a63a6dfecbb1b7))
* **go:** restore build error coverage ([1177c9c](https://github.com/rtk-ai/rtk/commit/1177c9c873ac63b6c0bcc9e1b664a705baa0ad7a))
* **grep:** close subprocess stdin to prevent memory leak ([#897](https://github.com/rtk-ai/rtk/issues/897)) ([7217562](https://github.com/rtk-ai/rtk/commit/72175623551f40b581b4a7f6ed966c1e4a9c7358))
* **grep:** close subprocess stdin to prevent memory leak ([#897](https://github.com/rtk-ai/rtk/issues/897)) ([09979cf](https://github.com/rtk-ai/rtk/commit/09979cf29701a1b775bcac761d24ec0e055d1bec))
* **hook_check:** detect missing integrations ([9cf9ccc](https://github.com/rtk-ai/rtk/commit/9cf9ccc1ac39f8bba37e932c7d318a3aa7a34ae9))
* **init:** remove opt-out instruction from telemetry message ([7571c8e](https://github.com/rtk-ai/rtk/commit/7571c8e101c41ee64c51e2bd64697f85f9142423))
* **init:** remove telemetry info lines from init output ([7dbef2c](https://github.com/rtk-ai/rtk/commit/7dbef2ce00824d26f2057e4c3c76e429e2e23088))
* **main:** kill zombie processes + path for rtk md ([d16fc6d](https://github.com/rtk-ai/rtk/commit/d16fc6dacbfec912c21522939b15b7bbd9719487))
* **main:** kill zombie processes + path for rtk md + missing intergrations ([a919335](https://github.com/rtk-ai/rtk/commit/a919335519ed4a5259a212e56407cb312aa99bac))
* **merge:** changelog conflicts ([d92c5d2](https://github.com/rtk-ai/rtk/commit/d92c5d264a49483c8d6079e04d946a79bc990a74))
* **proxy:** kill child process on SIGINT/SIGTERM to prevent orphans ([d813919](https://github.com/rtk-ai/rtk/commit/d813919a24546e044e7844fc7ed05fef4ec24033))
* **proxy:** kill child process on SIGINT/SIGTERM to prevent orphans ([3318510](https://github.com/rtk-ai/rtk/commit/33185101fc122d0c11a25a4e02ac9f3a7dc7e3bb))
* **review:** address ChildGuard disarm, stdin dedup, hook masking ([d85fe33](https://github.com/rtk-ai/rtk/commit/d85fe3384b87c16fafd25ec7bcadbff6e69f3f1f))
* **security:** default to ask when no permission rule matches ([#886](https://github.com/rtk-ai/rtk/issues/886)) ([158c745](https://github.com/rtk-ai/rtk/commit/158c74527f6591d372e40a78cd604d73a20649a9))
* **security:** default to ask when no permission rule matches ([#886](https://github.com/rtk-ai/rtk/issues/886)) ([41a6c6b](https://github.com/rtk-ai/rtk/commit/41a6c6bf6da78a4754794fdc6a1469df2e327920))
* **tracking:** use std::env::temp_dir() for compatibility (instead of unix tmp) ([e918661](https://github.com/rtk-ai/rtk/commit/e918661440d7b50321f0535032f52c5e87aaf3cb))

## [Unreleased]

### Features

* **aws:** expand CLI filters from 8 to 25 subcommands — CloudWatch Logs, CloudFormation events, Lambda, IAM, DynamoDB (with type unwrapping), ECS tasks, EC2 security groups, S3API objects, S3 sync/cp, EKS, SQS, Secrets Manager ([#885](https://github.com/rtk-ai/rtk/pull/885))
* **aws:** add shared runner `run_aws_filtered()` eliminating per-handler boilerplate
* **tee:** add `force_tee_hint()` — truncated output saves full data to file with recovery hint

## [0.34.3](https://github.com/rtk-ai/rtk/compare/v0.34.2...v0.34.3) (2026-04-02)


### Bug Fixes

* **automod:** add auto discovery for cmds ([234909d](https://github.com/rtk-ai/rtk/commit/234909d2c754ade2fdc939b0a1435a8e34ffc305))
* **ci:** fix validate-docs.sh broken module count check ([bbe3da6](https://github.com/rtk-ai/rtk/commit/bbe3da642b5fc4b065b13a65647ea0ebf5264e65))
* **cleaning:** constant extract ([aabc016](https://github.com/rtk-ai/rtk/commit/aabc0167bc013fd2d0c61a687580f6e69305500a))
* **cmds:** migrate remaining exit_code to exit_code_from_output ([ba9fa34](https://github.com/rtk-ai/rtk/commit/ba9fa345f3d1d14bd0af236ec9aa8a9a0e5581d6))
* **cmds:** more covering for run_filtered ([e48485a](https://github.com/rtk-ai/rtk/commit/e48485adc6a33d12b70664598020595cf7dfcd7e))
* **docs:** add documentation ([2f7278a](https://github.com/rtk-ai/rtk/commit/2f7278ac5992bf2e84b763fb05642d89900ba495))
* **docs:** add maintainers docs ([14265b4](https://github.com/rtk-ai/rtk/commit/14265b48c3a15e459a31da11250a51ab5830a508))
* **refacto-p1:** unified cmds execution flow  (+ rm dead code) ([75bd607](https://github.com/rtk-ai/rtk/commit/75bd607d55235f313855f5fe8c9eceafd73700a7))
* **refacto-p2:** more standardize ([47a76ea](https://github.com/rtk-ai/rtk/commit/47a76ea35ed2fe02a3600792163f727fa3a94ff2))
* **refacto-p2:** more standardize ([92c671a](https://github.com/rtk-ai/rtk/commit/92c671a175a5e2bf09720fd1a8591140bcb473a0))
* **refacto:** wrappers for standardization, exit codes lexer tokenizer, constants, code clean ([bff0258](https://github.com/rtk-ai/rtk/commit/bff02584243f1b73418418b0c05365acf56fbb36))
* **registry:** quoted env prefix + inline regex cleanup + routing docs ([f3217a4](https://github.com/rtk-ai/rtk/commit/f3217a467b543a3181605b257162f2b3ab5d5df0))
* **review:** address PR [#910](https://github.com/rtk-ai/rtk/issues/910) review feedback ([0a8b8fd](https://github.com/rtk-ai/rtk/commit/0a8b8fd0693fa504f376146cbbcafe9ddf4632c8))
* **review:** PR [#934](https://github.com/rtk-ai/rtk/issues/934) ([5bd35a3](https://github.com/rtk-ai/rtk/commit/5bd35a33ad6abe5278749726bed19912664531c2))
* **review:** PR [#934](https://github.com/rtk-ai/rtk/issues/934) ([bae7930](https://github.com/rtk-ai/rtk/commit/bae79301194bbb48d1cbb39554096c3225f7cb73))
* **rules:** add wc RtkRule with pattern field for develop compat ([d75e864](https://github.com/rtk-ai/rtk/commit/d75e864f20451a5e17918c75f2ea32672f65e1f4))
* **standardize:** git+kube sub wrappers run_filtered ([7fd221f](https://github.com/rtk-ai/rtk/commit/7fd221f44660bcf411aa333d2c35a49ff89e7961))
* **standardize:** merge pattern into rues ([08aabb9](https://github.com/rtk-ai/rtk/commit/08aabb95c3ae6e0b734f696264e1e1a8c0f0b22e))

## [0.34.2](https://github.com/rtk-ai/rtk/compare/v0.34.1...v0.34.2) (2026-03-30)


### Bug Fixes

* **emots:** replace 📊 with "Summary:" ([495a152](https://github.com/rtk-ai/rtk/commit/495a152059feabc7b516b96e804757608b87a10a))
* **refacto-codebase:** technical docs & sub folders ([927daef](https://github.com/rtk-ai/rtk/commit/927daef49b8f771d195201d196378e27e0ee8a2b))

## [0.34.1](https://github.com/rtk-ai/rtk/compare/v0.34.0...v0.34.1) (2026-03-28)


### Bug Fixes

* **security:** missing toml pkg ([51f9c88](https://github.com/rtk-ai/rtk/commit/51f9c888b81169309df92f7fa3a6f705d44adcab))
* **security:** salt device hash for telemetry ([32fdbbb](https://github.com/rtk-ai/rtk/commit/32fdbbbb6923c70d343fab14b4b0ce70424e610f))
* **security:** set 0600 permissions on salt file ([5eae11d](https://github.com/rtk-ai/rtk/commit/5eae11d16410dc4ff26e97672e5367b14efaab76))
* **telemetry:** cache salt in-process ([22dc059](https://github.com/rtk-ai/rtk/commit/22dc059310b0408adedc2d1228de339e16ea6c0a))
* **telemetry:** docs + real info from "rtk init -g" ([33195cc](https://github.com/rtk-ai/rtk/commit/33195cc686318ddcca54edfdd1215bd9fd28f891))
* **telemetry:** hash + salt ([92996b1](https://github.com/rtk-ai/rtk/commit/92996b127257eae16d3e17179592b2899f19254f))

## [0.34.0](https://github.com/rtk-ai/rtk/compare/v0.33.1...v0.34.0) (2026-03-26)


### Features

* **init:** add --copilot flag for GitHub Copilot integration ([9e19aac](https://github.com/rtk-ai/rtk/commit/9e19aac75e790ecbfd1dc5b2d01786f6b9edf506)), closes [#823](https://github.com/rtk-ai/rtk/issues/823)


### Bug Fixes

* **diff:** correct truncation overflow count in condense_unified_diff ([5399f83](https://github.com/rtk-ai/rtk/commit/5399f836a5c642121f0f6e7812ff4131d84d0509))
* **diff:** never truncate diff content — show all changes in full ([80fc29a](https://github.com/rtk-ai/rtk/commit/80fc29a839f51ef605474037e1a8fd86b4aac05a)), closes [#827](https://github.com/rtk-ai/rtk/issues/827)
* **git:** replace vague truncation markers with exact counts ([185fb97](https://github.com/rtk-ai/rtk/commit/185fb97061517922ea5844d8c6008f2eb86fd55d))
* **merge:** resolve conflict with develop in diff_cmd.rs ([6a5ae14](https://github.com/rtk-ai/rtk/commit/6a5ae1484b32c38bd99baca925175ae610e3d1e3))
* **read:** default to no filtering — show full file content ([5e0f3ba](https://github.com/rtk-ai/rtk/commit/5e0f3ba774eab52f8ca2ac603e2ae4eae79b2edc)), closes [#822](https://github.com/rtk-ai/rtk/issues/822)
* **read:** detect binary files and prevent empty output on filter failure ([8886c14](https://github.com/rtk-ai/rtk/commit/8886c14c9cf97fb4413efec3be8e50fdb84824e9)), closes [#822](https://github.com/rtk-ai/rtk/issues/822)
* rewrite swift test commands ([599ad25](https://github.com/rtk-ai/rtk/commit/599ad25deb0f8dc9ecab37f4bbe26324dac07b2e))
* truncation accuracy + Copilot init + binary file detection ([966bcbe](https://github.com/rtk-ai/rtk/commit/966bcbe638be18bbaba4298df985804643f82c85))
* **truncation:** accurate overflow counts and omission indicators ([58a9633](https://github.com/rtk-ai/rtk/commit/58a963347467613d48db05ad56bc8f1f3a06b65d))

## [Unreleased]

### Bug Fixes

* **wc:** `wc` filter was never invoked by the hook — removed `"wc "` from `IGNORED_PREFIXES` and added registry entry so `wc` commands are rewritten to `rtk wc`
* **diff:** correct truncation overflow count in condense_unified_diff ([#833](https://github.com/rtk-ai/rtk/pull/833)) ([5399f83](https://github.com/rtk-ai/rtk/commit/5399f83))
* **git:** replace vague truncation markers with exact counts in log and grep output ([#833](https://github.com/rtk-ai/rtk/pull/833)) ([185fb97](https://github.com/rtk-ai/rtk/commit/185fb97))

## [0.33.1](https://github.com/rtk-ai/rtk/compare/v0.33.0...v0.33.1) (2026-03-25)


### Bug Fixes

* **cicd:** dev- prefix for pre-release tags ([522bd64](https://github.com/rtk-ai/rtk/commit/522bd648c8cae41f6cadedcd40a96d879c6ecf0a))
* **cicd:** use dev- prefix for pre-release tags ([9c21275](https://github.com/rtk-ai/rtk/commit/9c212752fc0401820f8665198f00882684496175))
* **cicd:** use dev- prefix for pre-release tags to avoid polluting release-please ([32c67e0](https://github.com/rtk-ai/rtk/commit/32c67e01326374f0365602f61542a3639a8f121b))
* hook security + stderr redirects + version bump ([#807](https://github.com/rtk-ai/rtk/issues/807)) ([0649e97](https://github.com/rtk-ai/rtk/commit/0649e974fb8f27778ef0d22aa97905d9ebc8f03c))
* **hook:** respect Claude Code deny/ask permission rules on rewrite ([a051a6f](https://github.com/rtk-ai/rtk/commit/a051a6f5e56c7ee59375a365580bced634e29c02))
* strip trailing stderr redirects before rewrite matching ([#530](https://github.com/rtk-ai/rtk/issues/530)) ([edd9c02](https://github.com/rtk-ai/rtk/commit/edd9c02e892b297a7e349031b61ef971c982b53d))
* strip trailing stderr redirects before rewrite matching ([#530](https://github.com/rtk-ai/rtk/issues/530)) ([36a6f48](https://github.com/rtk-ai/rtk/commit/36a6f482296d6fc85f8116040a16de2e128733f8))

## [0.33.0-rc.54](https://github.com/rtk-ai/rtk/compare/v0.32.0-rc.54...v0.33.0-rc.54) (2026-03-24)


### Features

* **ruby:** add Ruby on Rails support (rspec, rubocop, rake, bundle) ([#724](https://github.com/rtk-ai/rtk/issues/724)) ([15bc0f8](https://github.com/rtk-ai/rtk/commit/15bc0f8d6e135371688d5fd42decc6d8a99454f0))


### Bug Fixes

* add telemetry documentation and init notice ([#640](https://github.com/rtk-ai/rtk/issues/640)) ([#788](https://github.com/rtk-ai/rtk/issues/788)) ([0eecee5](https://github.com/rtk-ai/rtk/commit/0eecee5bf35ffd8b13f36a59ec39bd52626948d3))
* **cargo:** preserve test compile diagnostics ([97b6878](https://github.com/rtk-ai/rtk/commit/97b68783f50d209c2c599ae42cc638520749e668))
* **cicd:** explicit fetch tag ([3b94b60](https://github.com/rtk-ai/rtk/commit/3b94b602ed24b9ecec597ce001e59f325caaadd4))
* **cicd:** gete release like tag for pre-release ([53bc81e](https://github.com/rtk-ai/rtk/commit/53bc81e9e6d3d0876fb1a23dbf6f08bc074b68be))
* **cicd:** issue 668 - pre release tag ([200af43](https://github.com/rtk-ai/rtk/commit/200af436d48dd2539cb00652b082f25c57873c9c))
* **cicd:** missing doc ([8657494](https://github.com/rtk-ai/rtk/commit/865749438e67f6da7f719d054bf377d857925ad3))
* **cicd:** pre-release correct tag ([1536667](https://github.com/rtk-ai/rtk/commit/15366678adeece701f38e91204128b070c0e3fc4))
* **dotnet:** TRX injection for Microsoft.Testing.Platform projects ([8eefef1](https://github.com/rtk-ai/rtk/commit/8eefef1b496035ce898effc5446e6851084d6fa4))
* **formatter:** show full error message for test failures ([#690](https://github.com/rtk-ai/rtk/issues/690)) ([dc6b026](https://github.com/rtk-ai/rtk/commit/dc6b0260ab4c1bdbccb4b775d879eb473b212c21))
* **formatter:** show full error message for test failures ([#690](https://github.com/rtk-ai/rtk/issues/690)) ([f7b09fc](https://github.com/rtk-ai/rtk/commit/f7b09fc86a693acf2b52954215ff0c4e6c5d03f9))
* **gh:** passthrough --comments flag in issue/pr view ([75cd223](https://github.com/rtk-ai/rtk/commit/75cd2232e274f898d8a335ba866fc507ce64b949))
* **gh:** passthrough --comments flag in issue/pr view ([fdeb09f](https://github.com/rtk-ai/rtk/commit/fdeb09fb93564e795711e9a531d2e2e20187c3a7)), closes [#720](https://github.com/rtk-ai/rtk/issues/720)
* **gh:** skip compact_diff for --name-only/--stat flags in pr diff ([2ef0690](https://github.com/rtk-ai/rtk/commit/2ef0690767eb733c705e4de56d02c64696a4acc6)), closes [#730](https://github.com/rtk-ai/rtk/issues/730)
* **gh:** skip compact_diff for --name-only/--stat in pr diff ([c576249](https://github.com/rtk-ai/rtk/commit/c57624931a96181f869645817fdd96bc056da044))
* **golangci-lint:** add v2 compatibility with runtime version detection ([95a4961](https://github.com/rtk-ai/rtk/commit/95a4961e4aa3ba5307b3dfad246c6168c4caeab8))
* **golangci:** use resolved_command for version detection, move test fixture to file ([6aa5e90](https://github.com/rtk-ai/rtk/commit/6aa5e90dc466f87c88a2401b4eb2aa0f323379f4))
* increase signal in git diff, git log, and json filters ([#621](https://github.com/rtk-ai/rtk/issues/621)) ([#708](https://github.com/rtk-ai/rtk/issues/708)) ([4edc3fc](https://github.com/rtk-ai/rtk/commit/4edc3fc0838e25ee6d1754c7e987b5507742f600))
* **playwright:** add tee_and_hint pass-through on failure ([#690](https://github.com/rtk-ai/rtk/issues/690)) ([b4ccf04](https://github.com/rtk-ai/rtk/commit/b4ccf046f59ce6ed1396e4d8c46f8a35152d6d09))
* preserve cargo test compile diagnostics ([15d5beb](https://github.com/rtk-ai/rtk/commit/15d5beb9f70caf1f84e9b506faaf840c70c1cf4e))
* **ruby:** use rails test for positional file args in rtk rake ([ec92c43](https://github.com/rtk-ai/rtk/commit/ec92c43f231eb2321a4b423b0eb8487f98161aac))
* **ruby:** use rails test for positional file args in rtk rake ([138e914](https://github.com/rtk-ai/rtk/commit/138e91411b4802e445a97429056cca73282d09e1))
* update Discord invite link ([#711](https://github.com/rtk-ai/rtk/issues/711)) ([#786](https://github.com/rtk-ai/rtk/issues/786)) ([af56573](https://github.com/rtk-ai/rtk/commit/af56573ae2b234123e4685fd945980e644f40fa3))

## [Unreleased]

### Bug Fixes

* **hook:** respect Claude Code deny/ask permission rules on rewrite — hook now checks settings.json before rewriting commands, preventing bypass of user-configured deny/ask permissions
* **git:** replace symbol prefixes (`* branch`, `+ Staged:`, `~ Modified:`, `? Untracked:`) with plain lowercase labels (`branch:`, `staged:`, `modified:`, `untracked:`) in git status output
* **ruby:** use `rails test` instead of `rake test` when positional file args are passed — `rake test` ignores positional files and only supports `TEST=path`

### Features

* **ruby:** add RSpec test runner filter with JSON parsing and text fallback (60%+ reduction)
* **ruby:** add RuboCop linter filter with JSON parsing, grouped by cop/severity (60%+ reduction)
* **ruby:** add Minitest filter for `rake test` / `rails test` with state machine parser (85-90% reduction)
* **ruby:** add TOML filter for `bundle install/update` — strip `Using` lines (90%+ reduction)
* **ruby:** add `ruby_exec()` shared utility for auto-detecting `bundle exec` when Gemfile exists
* **ruby:** add discover/rewrite rules for rake, rails, rspec, rubocop, and bundle commands

### Bug Fixes

* **cargo:** preserve compile diagnostics when `cargo test` fails before any test suites run
## [0.31.0](https://github.com/rtk-ai/rtk/compare/v0.30.1...v0.31.0) (2026-03-19)


### Features

* 9-tool AI agent support + emoji removal ([#704](https://github.com/rtk-ai/rtk/issues/704)) ([737dada](https://github.com/rtk-ai/rtk/commit/737dada4a56c0d7a482cc438e7280340d634f75d))

## [0.30.1](https://github.com/rtk-ai/rtk/compare/v0.30.0...v0.30.1) (2026-03-18)


### Bug Fixes

* remove all decorative emojis from CLI output ([#687](https://github.com/rtk-ai/rtk/issues/687)) ([#686](https://github.com/rtk-ai/rtk/issues/686)) ([4792008](https://github.com/rtk-ai/rtk/commit/4792008fc15553cbb9aeaa602f773a5f8f7f7afe))

## [0.30.0](https://github.com/rtk-ai/rtk/compare/v0.29.0...v0.30.0) (2026-03-16)


### Features

* add rtk session command for adoption overview ([be67d66](https://github.com/rtk-ai/rtk/commit/be67d660100c06a0751c08d943dc884ad5bff6a3))
* add rtk session command for adoption overview ([12d44c4](https://github.com/rtk-ai/rtk/commit/12d44c4068d7d4f65d5bd7551af29ab5a2352ed1)), closes [#487](https://github.com/rtk-ai/rtk/issues/487)
* add worktree slash commands for isolated development ([#364](https://github.com/rtk-ai/rtk/issues/364)) ([ab83e79](https://github.com/rtk-ai/rtk/commit/ab83e7933ebc26ca76f843d33285729875efb913))
* Claude Code tooling — 2 agents, 7 commands, 2 rules, 4 skills ([#491](https://github.com/rtk-ai/rtk/issues/491)) ([7b7a5ae](https://github.com/rtk-ai/rtk/commit/7b7a5ae4b6d23fbb882ed7d5e815e2ed0672c46c))


### Bug Fixes

* 6 critical bugs — exit codes, unwrap, lazy regex ([#626](https://github.com/rtk-ai/rtk/issues/626)) ([3005ebd](https://github.com/rtk-ai/rtk/commit/3005ebd0ad07912ae919687f6d3d49482aabaeac))
* align 7 TOML filter tests with on_empty behavior ([04ed6d8](https://github.com/rtk-ai/rtk/commit/04ed6d8c314dcbf86b147903b5a7f1cd956dc980))
* align 7 TOML filter tests with on_empty behavior ([9a499b9](https://github.com/rtk-ai/rtk/commit/9a499b9714e97a553d5603680ab1f843034acf28))
* **cicd-docs:** add agent reviewer + some contribute guidelines ([de710f4](https://github.com/rtk-ai/rtk/commit/de710f4ea30c333130c46f8a2e2c5b6b9edd4889))
* **cicd-docs:** some logs to understand what is happening when check docs ([191ea9a](https://github.com/rtk-ai/rtk/commit/191ea9af9f99ee78d74385fe1952ce83045e4afe))
* **cicd:** Clean cicd, rework depends and add pre-release ([d24a765](https://github.com/rtk-ai/rtk/commit/d24a7650e26aca89224a3ec5d263f1ce7c7121d6))
* **cicd:** Clean cicd, rework depends and add pre-release ([6303e95](https://github.com/rtk-ai/rtk/commit/6303e9530a379a8e3939e6c122ab4cf07cb16751))
* **cicd:** clippy - do not treat warn as error ([5da5db2](https://github.com/rtk-ai/rtk/commit/5da5db222d9927394995ccaeb3afc103e80c22bd))
* failing context for doc analyze -&gt; cat from files ([c6b7db2](https://github.com/rtk-ai/rtk/commit/c6b7db2e5a6cd9a05262e934b4fc7a44c699c3b0))
* git log --oneline regression drops commits ([#619](https://github.com/rtk-ai/rtk/issues/619)) ([8e85d67](https://github.com/rtk-ai/rtk/commit/8e85d676d78b12d2c421bb892f93971fc222fb39))
* improve adoption metric by detecting hook-rewritten commands ([eb8a2c4](https://github.com/rtk-ai/rtk/commit/eb8a2c4a71072870fca4b64e90189a4453acff84))
* normalize binlogs CRLF ([5344af9](https://github.com/rtk-ai/rtk/commit/5344af9a51f06b5dc42692e42c948ff11a3173c6))
* preserve commit body in git log output ([e189bbb](https://github.com/rtk-ai/rtk/commit/e189bbbe749120eda4d98a2130937269d8c0e92a))
* preserve first line of commit body in git log output ([c3416eb](https://github.com/rtk-ai/rtk/commit/c3416eb45f2f97297ec149d296a6a500697d302b))
* remove version check from validate-docs CI ([#476](https://github.com/rtk-ai/rtk/issues/476)) ([#543](https://github.com/rtk-ai/rtk/issues/543)) ([6e61c24](https://github.com/rtk-ai/rtk/commit/6e61c2447cc03af94220ce6ce83686f155e18086))
* split chained commands in adoption metric ([127f85c](https://github.com/rtk-ai/rtk/commit/127f85c02efd52a64e461005fa142d05f81615f8))
* support git -C &lt;path&gt; in rewrite registry ([c916bab](https://github.com/rtk-ai/rtk/commit/c916bab33ae9760b234fd720c944a849141f0d2e)), closes [#555](https://github.com/rtk-ai/rtk/issues/555)
* test-all.sh aborts when gt not installed ([#500](https://github.com/rtk-ai/rtk/issues/500)) ([#544](https://github.com/rtk-ai/rtk/issues/544)) ([26f5473](https://github.com/rtk-ai/rtk/commit/26f547371798ad32aed3569965303bc4857789ed))
* trust boundary followup — TOML key typo + missing meta commands ([#625](https://github.com/rtk-ai/rtk/issues/625)) ([8d8e188](https://github.com/rtk-ai/rtk/commit/8d8e188705e5784829693a83b2076d6118154764))
* windows path fix for git tests ([0a904e2](https://github.com/rtk-ai/rtk/commit/0a904e264d58f8f4b5f10e37ec3b11f717458fe0))

## [0.29.0](https://github.com/rtk-ai/rtk/compare/v0.28.2...v0.29.0) (2026-03-12)


### Features

* rewrite engine, OpenCode support, hook system improvements ([#539](https://github.com/rtk-ai/rtk/issues/539)) ([c1de10d](https://github.com/rtk-ai/rtk/commit/c1de10d94c0a35f825b71713e2db4624310c03d1))

## [0.28.2](https://github.com/rtk-ai/rtk/compare/v0.28.1...v0.28.2) (2026-03-10)


### Bug Fixes

* add tokens_saved to telemetry payload ([#471](https://github.com/rtk-ai/rtk/issues/471)) ([#472](https://github.com/rtk-ai/rtk/issues/472)) ([f8b7d52](https://github.com/rtk-ai/rtk/commit/f8b7d52d2d25d09a44f391576bad6a7b271f1f8c))

## [0.28.1](https://github.com/rtk-ai/rtk/compare/v0.28.0...v0.28.1) (2026-03-10)


### Bug Fixes

* 4 critical bugs + telemetry enrichment ([#462](https://github.com/rtk-ai/rtk/issues/462)) ([7d76af8](https://github.com/rtk-ai/rtk/commit/7d76af84b95e0f040e8b91a154edb89f80e5c380))
* restore lost telemetry install_method enrichment ([#469](https://github.com/rtk-ai/rtk/issues/469)) ([0c5cde9](https://github.com/rtk-ai/rtk/commit/0c5cde9ec234a2b7b0376adbcb78f2be48a98e86))

## [0.28.0](https://github.com/rtk-ai/rtk/compare/v0.27.2...v0.28.0) (2026-03-10)


### Features

* **gt:** add Graphite CLI support ([#290](https://github.com/rtk-ai/rtk/issues/290)) ([7fbc4ef](https://github.com/rtk-ai/rtk/commit/7fbc4ef4b553d5e61feeb6e73d8f6a96b6df3dd9))
* TOML Part 1 — filter DSL engine + 14 built-in filters ([#349](https://github.com/rtk-ai/rtk/issues/349)) ([adda253](https://github.com/rtk-ai/rtk/commit/adda2537be1fe69625ac280f15e8c8067d08c711))
* TOML Part 2 — user-global config, shadow warning, rtk init templates, 4 new built-in filters ([#351](https://github.com/rtk-ai/rtk/issues/351)) ([926e6a0](https://github.com/rtk-ai/rtk/commit/926e6a0dd4512c4cbb0f5ac133e60cb6134a3174))
* TOML Part 3 — 15 additional built-in filters (ping, rsync, dotnet, swift, shellcheck, hadolint, poetry, composer, brew, df, ps, systemctl, yamllint, markdownlint, uv) ([#386](https://github.com/rtk-ai/rtk/issues/386)) ([b71a8d2](https://github.com/rtk-ai/rtk/commit/b71a8d24e2dbd3ff9bb423c849638bfa23830c0b))

## [0.27.2](https://github.com/rtk-ai/rtk/compare/v0.27.1...v0.27.2) (2026-03-06)


### Bug Fixes

* gh pr edit/comment pass correct subcommand to gh ([#332](https://github.com/rtk-ai/rtk/issues/332)) ([799f085](https://github.com/rtk-ai/rtk/commit/799f0856e4547318230fe150a43f50ab82e1cf03))
* pass through -R/--repo flag in gh view commands ([#328](https://github.com/rtk-ai/rtk/issues/328)) ([0a1bcb0](https://github.com/rtk-ai/rtk/commit/0a1bcb05e5737311211369dcb92b3f756a6230c6)), closes [#223](https://github.com/rtk-ai/rtk/issues/223)
* reduce gh diff / git diff / gh api truncation ([#354](https://github.com/rtk-ai/rtk/issues/354)) ([#370](https://github.com/rtk-ai/rtk/issues/370)) ([e356c12](https://github.com/rtk-ai/rtk/commit/e356c1280da9896195d0dff91e152c5f20347a65))
* strip npx/bunx/pnpm prefixes in lint linter detection ([#186](https://github.com/rtk-ai/rtk/issues/186)) ([#366](https://github.com/rtk-ai/rtk/issues/366)) ([27b35d8](https://github.com/rtk-ai/rtk/commit/27b35d84a341622aa4bf686c2ce8867f8feeb742))

## [0.27.1](https://github.com/rtk-ai/rtk/compare/v0.27.0...v0.27.1) (2026-03-06)


### Bug Fixes

* only rewrite docker compose ps/logs/build, skip unsupported subcommands ([#336](https://github.com/rtk-ai/rtk/issues/336)) ([#363](https://github.com/rtk-ai/rtk/issues/363)) ([dbc9503](https://github.com/rtk-ai/rtk/commit/dbc950395e31b4b0bc48710dc52ad01d4d73f9ba))
* preserve -- separator for cargo commands and silence fallback ([#326](https://github.com/rtk-ai/rtk/issues/326)) ([45f9344](https://github.com/rtk-ai/rtk/commit/45f9344f033d27bc370ff54c4fc0c61e52446076)), closes [#286](https://github.com/rtk-ai/rtk/issues/286) [#287](https://github.com/rtk-ai/rtk/issues/287)
* prettier false positive when not installed ([#221](https://github.com/rtk-ai/rtk/issues/221)) ([#359](https://github.com/rtk-ai/rtk/issues/359)) ([85b0b3e](https://github.com/rtk-ai/rtk/commit/85b0b3eb0bad9cbacdc32d2e9ba525728acd7cbe))
* support git commit -am, --amend and other flags ([#327](https://github.com/rtk-ai/rtk/issues/327)) ([#360](https://github.com/rtk-ai/rtk/issues/360)) ([409aed6](https://github.com/rtk-ai/rtk/commit/409aed6dbcdd7cac2a48ec5655e6f1fd8d5248e3))

## [0.27.0](https://github.com/rtk-ai/rtk/compare/v0.26.0...v0.27.0) (2026-03-05)


### Features

* warn when installed hook is outdated ([#344](https://github.com/rtk-ai/rtk/issues/344)) ([#350](https://github.com/rtk-ai/rtk/issues/350)) ([3141fec](https://github.com/rtk-ai/rtk/commit/3141fecf958af5ae98c232543b913f3ca388254f))


### Bug Fixes

* bugs [#196](https://github.com/rtk-ai/rtk/issues/196) [#344](https://github.com/rtk-ai/rtk/issues/344) [#345](https://github.com/rtk-ai/rtk/issues/345) [#346](https://github.com/rtk-ai/rtk/issues/346) [#347](https://github.com/rtk-ai/rtk/issues/347) — gh --json, hook check, RTK_DISABLED, 2&gt;&1, json TOML ([8953af0](https://github.com/rtk-ai/rtk/commit/8953af0fc06759b37f16743ef383af0a52af2bed))
* RTK_DISABLED ignored, 2&gt;&1 broken, json TOML error ([#345](https://github.com/rtk-ai/rtk/issues/345), [#346](https://github.com/rtk-ai/rtk/issues/346), [#347](https://github.com/rtk-ai/rtk/issues/347)) ([6c13d23](https://github.com/rtk-ai/rtk/commit/6c13d234364d314f53b6698c282a621019635fd6))
* skip rewrite for gh --json/--jq/--template ([#196](https://github.com/rtk-ai/rtk/issues/196)) ([079ee9a](https://github.com/rtk-ai/rtk/commit/079ee9a4ea868ecf4e7beffcbc681ca1ba8b165c))

## [0.26.0](https://github.com/rtk-ai/rtk/compare/v0.25.0...v0.26.0) (2026-03-05)


### Features

* add Claude Code skills for PR and issue triage ([#343](https://github.com/rtk-ai/rtk/issues/343)) ([6ad6ffe](https://github.com/rtk-ai/rtk/commit/6ad6ffeccee9b622013f8e1357b6ca4c94aacb59))
* anonymous telemetry ping (1/day, opt-out) ([#334](https://github.com/rtk-ai/rtk/issues/334)) ([baff6a2](https://github.com/rtk-ai/rtk/commit/baff6a2334b155c0d68f38dba85bd8d6fe9e20af))


### Bug Fixes

* curl JSON size guard ([#297](https://github.com/rtk-ai/rtk/issues/297)) + exclude_commands config ([#243](https://github.com/rtk-ai/rtk/issues/243)) ([#342](https://github.com/rtk-ai/rtk/issues/342)) ([a8d6106](https://github.com/rtk-ai/rtk/commit/a8d6106f736e049013ecb77f0f413167266dd40e))

## [Unreleased]

### Features

* **toml-dsl:** declarative TOML filter engine — add command filters without writing Rust ([#299](https://github.com/rtk-ai/rtk/issues/299))
  * 8 primitives: `strip_ansi`, `replace`, `match_output`, `strip/keep_lines_matching`, `truncate_lines_at`, `head/tail_lines`, `max_lines`, `on_empty`
  * lookup chain: `.rtk/filters.toml` (project-local) → `~/.config/rtk/filters.toml` (user-global) → built-in filters
  * `RTK_NO_TOML=1` bypass, `RTK_TOML_DEBUG=1` debug mode
  * shadow warning when a TOML filter's match_command overlaps a Rust-handled command
  * `rtk init` generates commented filter templates at both project and global level
  * `rtk verify` command with `--require-all` for inline test validation
  * 18 built-in filters: `tofu-plan/init/validate/fmt` ([#240](https://github.com/rtk-ai/rtk/issues/240)), `du` ([#284](https://github.com/rtk-ai/rtk/issues/284)), `fail2ban-client` ([#281](https://github.com/rtk-ai/rtk/issues/281)), `iptables` ([#282](https://github.com/rtk-ai/rtk/issues/282)), `mix-format/compile` ([#310](https://github.com/rtk-ai/rtk/issues/310)), `shopify-theme` ([#280](https://github.com/rtk-ai/rtk/issues/280)), `pio-run` ([#231](https://github.com/rtk-ai/rtk/issues/231)), `mvn-build` ([#338](https://github.com/rtk-ai/rtk/issues/338)), `pre-commit`, `helm`, `gcloud`, `ansible-playbook`
* **hooks:** `exclude_commands` config — exclude specific commands from auto-rewrite ([#243](https://github.com/rtk-ai/rtk/issues/243))

### Bug Fixes

* **cargo clippy:** include actionable error details in compact output instead of summary-only counts ([#602](https://github.com/rtk-ai/rtk/issues/602))
* **curl:** skip JSON schema replacement when schema is larger than original payload ([#297](https://github.com/rtk-ai/rtk/issues/297))
* **toml-dsl:** fix regex overmatch on `tofu-plan/init/validate/fmt` and `mix-format/compile` — add `(\s|$)` word boundary to prevent matching subcommands (e.g. `tofu planet`, `mix formats`) ([#349](https://github.com/rtk-ai/rtk/issues/349))
* **toml-dsl:** remove 3 dead built-in filters (`docker-inspect`, `docker-compose-ps`, `pnpm-build`) — Clap routes these commands before `run_fallback`, so the TOML filters never fire ([#351](https://github.com/rtk-ai/rtk/issues/351))
* **toml-dsl:** `uv-sync` — remove `Resolved` short-circuit; it fires before the package list is printed, hiding installed packages ([#386](https://github.com/rtk-ai/rtk/issues/386))
* **toml-dsl:** `dotnet-build` — short-circuit only when both warning and error counts are zero; builds with warnings now pass through ([#386](https://github.com/rtk-ai/rtk/issues/386))
* **toml-dsl:** `poetry-install` — support Poetry 2.x bullet syntax (`•`) and `No changes.` up-to-date message ([#386](https://github.com/rtk-ai/rtk/issues/386))
* **toml-dsl:** `ping` — add Windows format support (`Pinging` header, `Reply from` per-packet lines) ([#386](https://github.com/rtk-ai/rtk/issues/386))

## [0.25.0](https://github.com/rtk-ai/rtk/compare/v0.24.0...v0.25.0) (2026-03-05)


### Features

* `rtk rewrite` — single source of truth for LLM hook rewrites ([#241](https://github.com/rtk-ai/rtk/issues/241)) ([f447a3d](https://github.com/rtk-ai/rtk/commit/f447a3d5b136dd5b1df3d5cc4969e29a68ba3f89))


### Bug Fixes

* **find:** accept native find flags (-name, -type, etc.) ([#211](https://github.com/rtk-ai/rtk/issues/211)) ([7ac5bc4](https://github.com/rtk-ai/rtk/commit/7ac5bc4bd3942841cc1abb53399025b4fcae10c9))

## [Unreleased]

### ⚠️ Migration Required

**Hook must be updated after upgrading** (`rtk init --global`).

The Claude Code hook is now a thin delegator: all rewrite logic lives in the
`rtk rewrite` command (single source of truth). The old hook embedded the full
if-else mapping inline — it still works after upgrading, but won't pick up new
commands automatically.

**Upgrade path:**
```bash
cargo install rtk          # upgrade binary
rtk init --global          # replace old hook with thin delegator
```

Running `rtk init` without `--global` updates the project-level hook only.
Users who skip this step keep the old hook working as before — no immediate
breakage, but future rule additions won't take effect until they migrate.

### Features

* **rewrite**: add `rtk rewrite` command — single source of truth for hook rewrites ([#241](https://github.com/rtk-ai/rtk/pull/241))
  - New `src/discover/registry.rs` handles all command → RTK mapping
  - Hook reduced to ~50 lines (thin delegator), no duplicate logic
  - New commands automatically available in hook without hook file changes
  - Supports compound commands (`&&`, `||`, `;`, `|`, `&`) and env prefixes
* **discover**: extract rules/patterns into `src/discover/rules.rs` — adding a command now means editing one file only
* **fix**: add `aws` and `psql` to rewrite registry (were missing despite modules existing since 0.24.0)

### Tests

* +48 regression tests covering all command categories: aws, psql, Python, Go, JS/TS,
  compound operators, sudo/env prefixes, registry invariants (607 total, was 559)

## [0.24.0](https://github.com/rtk-ai/rtk/compare/v0.23.0...v0.24.0) (2026-03-04)


### Features

* add AWS CLI and psql modules with token-optimized output ([#216](https://github.com/rtk-ai/rtk/issues/216)) ([b934466](https://github.com/rtk-ai/rtk/commit/b934466364c131de2656eefabe933965f8424e18))
* passthrough fallback when Clap parse fails + review fixes ([#200](https://github.com/rtk-ai/rtk/issues/200)) ([772b501](https://github.com/rtk-ai/rtk/commit/772b5012ede833c3f156816f212d469560449a30))
* **security:** add SHA-256 hook integrity verification ([f2caca3](https://github.com/rtk-ai/rtk/commit/f2caca3abc330fb45a466af6a837ed79c3b00b40))


### Bug Fixes

* **git:** propagate exit codes in push/pull/fetch/stash/worktree ([#234](https://github.com/rtk-ai/rtk/issues/234)) ([5cfaecc](https://github.com/rtk-ai/rtk/commit/5cfaeccaba2fc6e1fe5284f57b7af7ec7c0a224d))
* **playwright:** fix JSON parser to match real Playwright output format ([#193](https://github.com/rtk-ai/rtk/issues/193)) ([4eb6cf4](https://github.com/rtk-ai/rtk/commit/4eb6cf4b1a2333cb710970e40a96f1004d4ab0fa))
* support additional git global options (--no-pager, --no-optional-locks, --bare, --literal-pathspecs) ([68ca712](https://github.com/rtk-ai/rtk/commit/68ca7126d45609a41dbff95e2770d58a11ebc0a3))
* support git global options (-C, -c, --git-dir, --work-tree, --no-pager, --no-optional-locks, --bare, --literal-pathspecs) ([a6ccefe](https://github.com/rtk-ai/rtk/commit/a6ccefe8e71372b61e6e556f0d36a944d1bcbd70))
* support git global options (-C, -c, --git-dir, --work-tree) ([982084e](https://github.com/rtk-ai/rtk/commit/982084ee34c17d2fe89ff9f4839374bf0caa2d19))
* update version refs to 0.23.0, module count to 51, fmt upstream files ([eed0188](https://github.com/rtk-ai/rtk/commit/eed018814b141ada8140f350adc26d9f104cf368))

## [0.23.0](https://github.com/rtk-ai/rtk/compare/v0.22.2...v0.23.0) (2026-02-28)


### Features

* add mypy command with grouped error output ([#109](https://github.com/rtk-ai/rtk/issues/109)) ([e8ef341](https://github.com/rtk-ai/rtk/commit/e8ef3418537247043808dc3c88bfd189b717a0a1))
* **gain:** add per-project token savings with -p flag ([#128](https://github.com/rtk-ai/rtk/issues/128)) ([2b550ee](https://github.com/rtk-ai/rtk/commit/2b550eebd6219a4844488d8fde1842ba3c6dec25))


### Bug Fixes

* eliminate duplicate output when grep-ing function names from git show ([#248](https://github.com/rtk-ai/rtk/issues/248)) ([a6f65f1](https://github.com/rtk-ai/rtk/commit/a6f65f11da71936d148a2562216ab45b4c4b04a0))
* filter docker compose hook rewrites to supported subcommands ([#245](https://github.com/rtk-ai/rtk/issues/245)) ([dbbf980](https://github.com/rtk-ai/rtk/commit/dbbf980f3ba9a51d0f7eb703e7b3c52fde2b784f)), closes [#244](https://github.com/rtk-ai/rtk/issues/244)
* **registry:** "fi" in IGNORED_PREFIXES shadows find commands ([#246](https://github.com/rtk-ai/rtk/issues/246)) ([48965c8](https://github.com/rtk-ai/rtk/commit/48965c85d2dd274bbdcf27b11850ccd38909e6f4))
* remove personal preferences from project CLAUDE.md ([3a8044e](https://github.com/rtk-ai/rtk/commit/3a8044ef6991b2208d904b7401975fcfcb165cdb))
* remove personal preferences from project CLAUDE.md ([d362ad0](https://github.com/rtk-ai/rtk/commit/d362ad0e4968cfc6aa93f9ef163512a692ca5d1b))
* remove remaining personal project reference from CLAUDE.md ([5b59700](https://github.com/rtk-ai/rtk/commit/5b597002dcd99029cb9c0da9b6d38b44021bdb3a))
* remove remaining personal project reference from CLAUDE.md ([dc09265](https://github.com/rtk-ai/rtk/commit/dc092655fb84a7c19a477e731eed87df5ad0b89f))
* surface build failures in go test summary ([#274](https://github.com/rtk-ai/rtk/issues/274)) ([b405e48](https://github.com/rtk-ai/rtk/commit/b405e48ca6c4be3ba702a5d9092fa4da4dff51dc))

## [0.22.2](https://github.com/rtk-ai/rtk/compare/v0.22.1...v0.22.2) (2026-02-20)


### Bug Fixes

* **grep:** accept -n flag for grep/rg compatibility ([7d561cc](https://github.com/rtk-ai/rtk/commit/7d561cca51e4e177d353e6514a618e5bb09eebc6))
* **playwright:** fix JSON parser and binary resolution ([#215](https://github.com/rtk-ai/rtk/issues/215)) ([461856c](https://github.com/rtk-ai/rtk/commit/461856c8fd78cce8e2d875ae878111d7cb3610cd))
* propagate rg exit code in rtk grep for CLI parity ([#227](https://github.com/rtk-ai/rtk/issues/227)) ([f1be885](https://github.com/rtk-ai/rtk/commit/f1be88565e602d3b6777f629d417e957a62daae2)), closes [#162](https://github.com/rtk-ai/rtk/issues/162)

## [0.22.1](https://github.com/rtk-ai/rtk/compare/v0.22.0...v0.22.1) (2026-02-19)


### Bug Fixes

* git branch creation silently swallowed by list mode ([#194](https://github.com/rtk-ai/rtk/issues/194)) ([88dc752](https://github.com/rtk-ai/rtk/commit/88dc752220dc79dfa09b871065b28ae6ef907231))
* **git:** support multiple -m flags in git commit ([292225f](https://github.com/rtk-ai/rtk/commit/292225f2dd09bfc5274cc8b4ed92d1a519929629))
* **git:** support multiple -m flags in git commit ([c18553a](https://github.com/rtk-ai/rtk/commit/c18553a55c1192610525a5341a183da46c59d50c))
* **grep:** translate BRE \| alternation and strip -r flag for rg ([#206](https://github.com/rtk-ai/rtk/issues/206)) ([70d1b04](https://github.com/rtk-ai/rtk/commit/70d1b04093a3dfcc99991502f1530cbb13bae872))
* propagate linter exit code in rtk lint ([#207](https://github.com/rtk-ai/rtk/issues/207)) ([8e826fc](https://github.com/rtk-ai/rtk/commit/8e826fc89fe7350df82ee2b1bae8104da609f2b2)), closes [#185](https://github.com/rtk-ai/rtk/issues/185)
* smart markdown body filter for gh issue/pr view ([#188](https://github.com/rtk-ai/rtk/issues/188)) ([#214](https://github.com/rtk-ai/rtk/issues/214)) ([4208015](https://github.com/rtk-ai/rtk/commit/4208015cce757654c150f3d71ddd004d22b4dd25))

## [0.22.0](https://github.com/rtk-ai/rtk/compare/v0.21.1...v0.22.0) (2026-02-18)


### Features

* add `rtk wc` command for compact word/line/byte counts ([#175](https://github.com/rtk-ai/rtk/issues/175)) ([393fa5b](https://github.com/rtk-ai/rtk/commit/393fa5ba2bda0eb1f8655a34084ea4c1e08070ae))

## [0.21.1](https://github.com/rtk-ai/rtk/compare/v0.21.0...v0.21.1) (2026-02-17)


### Bug Fixes

* gh run view drops --log-failed, --log, --json flags ([#159](https://github.com/rtk-ai/rtk/issues/159)) ([d196c2d](https://github.com/rtk-ai/rtk/commit/d196c2d2df9b7a807e02ace557a4eea45cfee77d))

## [0.21.0](https://github.com/rtk-ai/rtk/compare/v0.20.1...v0.21.0) (2026-02-17)


### Features

* **docker:** add docker compose support ([#110](https://github.com/rtk-ai/rtk/issues/110)) ([510c491](https://github.com/rtk-ai/rtk/commit/510c491238731b71b58923a0f20443ade6df5ae7))

## [0.20.1](https://github.com/rtk-ai/rtk/compare/v0.20.0...v0.20.1) (2026-02-17)


### Bug Fixes

* install to ~/.local/bin instead of /usr/local/bin (closes [#155](https://github.com/rtk-ai/rtk/issues/155)) ([#161](https://github.com/rtk-ai/rtk/issues/161)) ([0b34772](https://github.com/rtk-ai/rtk/commit/0b34772a679f3c6b5dd9609af2f6eec6d79e4a64))

## [0.20.0](https://github.com/rtk-ai/rtk/compare/v0.19.0...v0.20.0) (2026-02-16)


### Features

* add hook audit mode for verifiable rewrite metrics ([#151](https://github.com/rtk-ai/rtk/issues/151)) ([70c3786](https://github.com/rtk-ai/rtk/commit/70c37867e7282ee0ccf200022ecef8c6e4ab52f4))

## [0.19.0](https://github.com/rtk-ai/rtk/compare/v0.18.1...v0.19.0) (2026-02-16)


### Features

* tee raw output to file for LLM re-read without re-run ([#134](https://github.com/rtk-ai/rtk/issues/134)) ([a08a62b](https://github.com/rtk-ai/rtk/commit/a08a62b4e3b3c6a2ad933978b1143dcfc45cf891))

## [0.18.1](https://github.com/rtk-ai/rtk/compare/v0.18.0...v0.18.1) (2026-02-15)


### Bug Fixes

* update ARCHITECTURE.md version to 0.18.0 ([398cb08](https://github.com/rtk-ai/rtk/commit/398cb08125410a4de11162720cf3499d3c76f12d))
* update version references to 0.16.0 in README.md and CLAUDE.md ([ec54833](https://github.com/rtk-ai/rtk/commit/ec54833621c8ca666735e1a08ed5583624b250c1))
* update version references to 0.18.0 in docs ([c73ed47](https://github.com/rtk-ai/rtk/commit/c73ed470a79ab9e4771d2ad65394859e672b4123))

## [0.18.0](https://github.com/rtk-ai/rtk/compare/v0.17.0...v0.18.0) (2026-02-15)


### Features

* **gain:** colored dashboard with efficiency meter and impact bars ([#129](https://github.com/rtk-ai/rtk/issues/129)) ([606b86e](https://github.com/rtk-ai/rtk/commit/606b86ed43902dc894e6f1711f6fe7debedc2530))

## [0.17.0](https://github.com/rtk-ai/rtk/compare/v0.16.0...v0.17.0) (2026-02-15)


### Features

* **cargo:** add cargo nextest support with failures-only output ([#107](https://github.com/rtk-ai/rtk/issues/107)) ([68fd570](https://github.com/rtk-ai/rtk/commit/68fd570f2b7d5aaae7b37b07eb24eae21542595e))
* **hook:** handle global options before subcommands ([#99](https://github.com/rtk-ai/rtk/issues/99)) ([7401f10](https://github.com/rtk-ai/rtk/commit/7401f1099f3ef14598f11947262756e3f19fce8f))

## [0.16.0](https://github.com/rtk-ai/rtk/compare/v0.15.4...v0.16.0) (2026-02-14)


### Features

* **python:** add lint dispatcher + universal format command ([#100](https://github.com/rtk-ai/rtk/issues/100)) ([4cae6b6](https://github.com/rtk-ai/rtk/commit/4cae6b6c9a4fbc91c56a99f640d217478b92e6d9))

## [0.15.4](https://github.com/rtk-ai/rtk/compare/v0.15.3...v0.15.4) (2026-02-14)


### Bug Fixes

* **git:** fix for issue [#82](https://github.com/rtk-ai/rtk/issues/82) ([04e6bb0](https://github.com/rtk-ai/rtk/commit/04e6bb032ccd67b51fb69e326e27eff66c934043))
* **git:** Returns "Not a git repository" when git status is executed in a non-repo folder [#82](https://github.com/rtk-ai/rtk/issues/82) ([d4cb2c0](https://github.com/rtk-ai/rtk/commit/d4cb2c08100d04755fa776ec8000c0b9673e4370))

## [0.15.3](https://github.com/rtk-ai/rtk/compare/v0.15.2...v0.15.3) (2026-02-13)


### Bug Fixes

* prevent UTF-8 panics on multi-byte characters ([#93](https://github.com/rtk-ai/rtk/issues/93)) ([155e264](https://github.com/rtk-ai/rtk/commit/155e26423d1fe2acbaed3dc1aab8c365324d53e0))

## [0.15.2](https://github.com/rtk-ai/rtk/compare/v0.15.1...v0.15.2) (2026-02-13)


### Bug Fixes

* **hook:** use POSIX character classes for cross-platform grep compatibility ([#98](https://github.com/rtk-ai/rtk/issues/98)) ([4aafc83](https://github.com/rtk-ai/rtk/commit/4aafc832d4bdd438609358e2737a96bee4bb2467))

## [0.15.1](https://github.com/rtk-ai/rtk/compare/v0.15.0...v0.15.1) (2026-02-12)


### Bug Fixes

* improve CI reliability and hook coverage ([#95](https://github.com/rtk-ai/rtk/issues/95)) ([ac80bfa](https://github.com/rtk-ai/rtk/commit/ac80bfa88f91dfaf562cdd786ecd3048c554e4f7))
* **vitest:** robust JSON extraction for pnpm/dotenv prefixes ([#92](https://github.com/rtk-ai/rtk/issues/92)) ([e5adba8](https://github.com/rtk-ai/rtk/commit/e5adba8b214a6609cf1a2cda05f21bcf2a1adb94))

## [0.15.0](https://github.com/rtk-ai/rtk/compare/v0.14.0...v0.15.0) (2026-02-12)


### Features

* add Python and Go support ([#88](https://github.com/rtk-ai/rtk/issues/88)) ([a005bb1](https://github.com/rtk-ai/rtk/commit/a005bb15c030e16b7b87062317bddf50e12c6f32))
* **cargo:** aggregate test output into single line ([#83](https://github.com/rtk-ai/rtk/issues/83)) ([#85](https://github.com/rtk-ai/rtk/issues/85)) ([06b1049](https://github.com/rtk-ai/rtk/commit/06b10491f926f9eca4323c80d00530a1598ec649))
* make install-local.sh self-contained ([#89](https://github.com/rtk-ai/rtk/issues/89)) ([b82ad16](https://github.com/rtk-ai/rtk/commit/b82ad168533881757f45e28826cb0c4bd4cc6f97))

## [0.14.0](https://github.com/rtk-ai/rtk/compare/v0.13.1...v0.14.0) (2026-02-12)


### Features

* **ci:** automate Homebrew formula update on release ([#80](https://github.com/rtk-ai/rtk/issues/80)) ([a0d2184](https://github.com/rtk-ai/rtk/commit/a0d2184bfef4d0a05225df5a83eedba3c35865b3))


### Bug Fixes

* add website URL (rtk-ai.app) across project metadata ([#81](https://github.com/rtk-ai/rtk/issues/81)) ([c84fa3c](https://github.com/rtk-ai/rtk/commit/c84fa3c060c7acccaedb617852938c894f30f81e))
* update stale repo URLs from pszymkowiak/rtk to rtk-ai/rtk ([#78](https://github.com/rtk-ai/rtk/issues/78)) ([55d010a](https://github.com/rtk-ai/rtk/commit/55d010ad5eced14f525e659f9f35d051644a1246))

## [0.13.1](https://github.com/rtk-ai/rtk/compare/v0.13.0...v0.13.1) (2026-02-12)


### Bug Fixes

* **ci:** fix release artifacts not uploading ([#73](https://github.com/rtk-ai/rtk/issues/73)) ([bb20b1e](https://github.com/rtk-ai/rtk/commit/bb20b1e9e1619e0d824eb0e0b87109f30bf4f513))
* **ci:** fix release workflow not uploading artifacts to GitHub releases ([bd76b36](https://github.com/rtk-ai/rtk/commit/bd76b361908d10cce508aff6ac443340dcfbdd76))

## [0.13.0](https://github.com/rtk-ai/rtk/compare/v0.12.0...v0.13.0) (2026-02-12)


### Features

* **sqlite:** add custom sqlite db location ([6e181ae](https://github.com/rtk-ai/rtk/commit/6e181aec087edb50625e08b72fe7abdadbb6c72b))
* **sqlite:** add custom sqlite db location ([93364b5](https://github.com/rtk-ai/rtk/commit/93364b5457619201c656fc2423763fea77633f15))

## [0.12.0](https://github.com/rtk-ai/rtk/compare/v0.11.0...v0.12.0) (2026-02-09)


### Features

* **cargo:** add `cargo install` filtering with 80-90% token reduction ([645a773](https://github.com/rtk-ai/rtk/commit/645a773a65bb57dc2635aa405a6e2b87534491e3)), closes [#69](https://github.com/rtk-ai/rtk/issues/69)
* **cargo:** add cargo install filtering ([447002f](https://github.com/rtk-ai/rtk/commit/447002f8ba3bbd2b398f85db19b50982df817a02))

## [0.11.0](https://github.com/rtk-ai/rtk/compare/v0.10.0...v0.11.0) (2026-02-07)


### Features

* **init:** auto-patch settings.json for frictionless hook installation ([2db7197](https://github.com/rtk-ai/rtk/commit/2db7197e020857c02857c8ef836279c3fd660baf))

## [Unreleased]

### Added
- **settings.json auto-patch** for frictionless hook installation
  - Default `rtk init -g` now prompts to patch settings.json [y/N]
  - `--auto-patch`: Patch immediately without prompting (CI/CD workflows)
  - `--no-patch`: Skip patching, print manual instructions instead
  - Automatic backup: creates `settings.json.bak` before modification
  - Idempotent: detects existing hook, skips modification if present
  - `rtk init --show` now displays settings.json status
- **Uninstall command** for complete RTK removal
  - `rtk init -g --uninstall` removes hook, RTK.md, CLAUDE.md reference, and settings.json entry
  - Restores clean state for fresh installation or testing
- **Improved error handling** with detailed context messages
  - All error messages now include file paths and actionable hints
  - UTF-8 validation for hook paths
  - Disk space hints on write failures

### Changed
- Refactored `insert_hook_entry()` to use idiomatic Rust `entry()` API
- Simplified `hook_already_present()` logic with iterator chains
- Improved atomic write error messages for better debugging
## [0.10.0](https://github.com/rtk-ai/rtk/compare/v0.9.4...v0.10.0) (2026-02-07)


### Features

* Hook-first installation with 99.5% token reduction ([e7f80ad](https://github.com/rtk-ai/rtk/commit/e7f80ad29481393d16d19f55b3c2171a4b8b7915))
* **init:** refactor to hook-first with slim RTK.md ([9620f66](https://github.com/rtk-ai/rtk/commit/9620f66cd64c299426958d4d3d65bd8d1a9bc92d))

## [0.9.4](https://github.com/rtk-ai/rtk/compare/v0.9.3...v0.9.4) (2026-02-06)


### Bug Fixes

* **discover:** add cargo check support, wire RtkStatus::Passthrough, enhance rtk init ([d5f8a94](https://github.com/rtk-ai/rtk/commit/d5f8a9460421821861a32eedefc0800fb7720912))

## [0.9.3](https://github.com/rtk-ai/rtk/compare/v0.9.2...v0.9.3) (2026-02-06)


### Bug Fixes

* P0 crashes + cargo check + dedup utilities + discover status ([05078ff](https://github.com/rtk-ai/rtk/commit/05078ff2dab0c8745b9fb44b1d462c0d32ae8d77))
* P0 crashes + cargo check + dedup utilities + discover status ([60d2d25](https://github.com/rtk-ai/rtk/commit/60d2d252efbedaebae750b3122385b2377ab01eb))

## [0.9.2](https://github.com/rtk-ai/rtk/compare/v0.9.1...v0.9.2) (2026-02-05)


### Bug Fixes

* **git:** accept native git flags in add command (including -A) ([2ade8fe](https://github.com/rtk-ai/rtk/commit/2ade8fe030d8b1bc2fa294aa710ed1f5f877136f))
* **git:** accept native git flags in add command (including -A) ([40e7ead](https://github.com/rtk-ai/rtk/commit/40e7eadbaf0b89a54b63bea73014eac7cf9afb05))

## [0.9.1](https://github.com/rtk-ai/rtk/compare/v0.9.0...v0.9.1) (2026-02-04)


### Bug Fixes

* **tsc:** show every TypeScript error instead of collapsing by code ([3df8ce5](https://github.com/rtk-ai/rtk/commit/3df8ce552585d8d0a36f9c938d381ac0bc07b220))
* **tsc:** show every TypeScript error instead of collapsing by code ([67e8de8](https://github.com/rtk-ai/rtk/commit/67e8de8732363d111583e5b514d05e092355b97e))

## [0.9.0](https://github.com/rtk-ai/rtk/compare/v0.8.1...v0.9.0) (2026-02-03)


### Features

* add rtk tree + fix rtk ls + audit phase 1-2 ([278cc57](https://github.com/rtk-ai/rtk/commit/278cc5700bc39770841d157f9c53161f8d62df1e))
* audit phase 3 + tracking validation + rtk learn ([7975624](https://github.com/rtk-ai/rtk/commit/7975624d0a83c44dfeb073e17fd07dbc62dc8329))
* **git:** add fallback passthrough for unsupported subcommands ([32bbd02](https://github.com/rtk-ai/rtk/commit/32bbd025345872e46f67e8c999ecc6f71891856b))
* **grep:** add extra args passthrough (-i, -A/-B/-C, etc.) ([a240d1a](https://github.com/rtk-ai/rtk/commit/a240d1a1ee0d94c178d0c54b411eded6c7839599))
* **pnpm:** add fallback passthrough for unsupported subcommands ([614ff5c](https://github.com/rtk-ai/rtk/commit/614ff5c13f526f537231aaa9fa098763822b4ee0))
* **read:** add stdin support via "-" path ([060c38b](https://github.com/rtk-ai/rtk/commit/060c38b3c1ab29070c16c584ea29da3d5ca28f3d))
* rtk tree + fix rtk ls + full audit (phase 1-2-3) ([cb83da1](https://github.com/rtk-ai/rtk/commit/cb83da104f7beba3035225858d7f6eb2979d950c))


### Bug Fixes

* **docs:** escape HTML tags in rustdoc comments ([b13d92c](https://github.com/rtk-ai/rtk/commit/b13d92c9ea83e28e97847e0a6da696053364bbfc))
* **find:** rewrite with ignore crate + fix json stdin + benchmark pipeline ([fcc1462](https://github.com/rtk-ai/rtk/commit/fcc14624f89a7aa9742de4e7bc7b126d6d030871))
* **ls:** compact output (-72% tokens) + fix discover panic ([ea7cdb7](https://github.com/rtk-ai/rtk/commit/ea7cdb7a3b622f62e0a085144a637a22108ffdb7))

## [0.8.1](https://github.com/rtk-ai/rtk/compare/v0.8.0...v0.8.1) (2026-02-02)


### Bug Fixes

* allow git status to accept native flags ([a7ea143](https://github.com/rtk-ai/rtk/commit/a7ea1439fb99a9bd02292068625bed6237f6be0c))
* allow git status to accept native flags ([a27bce8](https://github.com/rtk-ai/rtk/commit/a27bce82f09701cb9df2ed958f682ab5ac8f954e))

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

- **Repository**: https://github.com/rtk-ai/rtk (maintained by pszymkowiak)
- **Issues**: https://github.com/rtk-ai/rtk/issues

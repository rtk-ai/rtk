# PHP

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `php_cmd.rs` summarizes `php -l` syntax checks and routes `php artisan*` to specialized helpers
- `artisan_cmd.rs` cleans Artisan output and applies runner-aware filtering for `php artisan test`
- `phpunit_cmd.rs` strips progress/header noise and keeps failure details + final summary
- `phpstan_cmd.rs` injects JSON output by default and emits compact file/line error summaries
- `ecs_cmd.rs` condenses EasyCodingStandard output while preserving file paths and error lines
- `pest_cmd.rs` runs Pest with compact progress suppression and test-focused output
- `paratest_cmd.rs` runs ParaTest with compact progress suppression and test-focused output
- `phpcs_cmd.rs` runs PHP_CodeSniffer (`phpcs`) and its fixer (`phpcbf`); drops report frames (dash rows, blank lines, `-p` progress, `Time:` footer) and keeps every finding. `--report=json|xml|...`, `-v`, `-i`, `-e` and stdin runs pass through
- `test_output.rs` provides shared PHPUnit/Pest/ParaTest output filtering logic
- `utils.rs` resolves local `vendor/bin/*` tools first, then falls back to global binaries

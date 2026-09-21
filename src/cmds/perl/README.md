# Perl

> Part of [`src/cmds/`](../README.md). See also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md).

## Specifics

- `tap.rs` holds the shared filter for the console output of `TAP::Harness`. If a test file failed or printed something that is not TAP, it keeps that file's block. Otherwise it drops the block. It keeps `not ok` lines, diagnostics, die messages and compile errors in their file's block, and keeps the summary report. It drops passing TAP, plans, subtest headers, TODO failures, wait statuses and CPU times. `filter_harness_run()` also drops MakeMaker and Module::Build chatter, for tools that run `make test`.
- `prove_cmd.rs` adds `-v -m` to prove, unless the user passed `-q` or `-Q`. With both flags, each test's STDERR arrives inside its own file's block, in order, even under `-j`. Without them, rtk captures stdout and stderr apart, and every diagnostic ends up after the summary. `--formatter`, `--harness`, `-D` and help pass through.
- `yath_cmd.rs` groups yath's per-job event lines. It prints each failed job once, with its assertions, diagnostics, STDERR and `REASON` lines. It sets `TABLE_TERM_SIZE` so that the job tables do not wrap file names.
- `perlcritic_cmd.rs` passes its own tab-separated `--verbose` format, because the default format and `--verbose 8` can drop the file name. It prints one line per file, policy and message, with every `line:column`, most severe first. A user `--verbose` and the listing modes pass through. Output over `CAP_INVENTORY` lines goes to `rtk recall`.
- `cover_cmd.rs` keeps only what is not covered. The summary table keeps its header, the Total row and each file under 100%. The text report keeps the missed lines, the missed branch and condition rows, and the uncovered subroutines. `cover -test` output goes through `filter_harness_run()`.
- `dzil_cmd.rs` drops `[DZ]` progress lines and keeps every other plugin line and error. `dzil test` output goes through `filter_harness_run()`.
- `perldoc_cmd.rs` adds `-T` and drops the AUTHOR, MAINTAINERS, COPYRIGHT, LICENSE, BUGS, SUPPORT, SOURCE and HISTORY sections. Documentation is the payload, so this filter does not aim for 60%.
- The hook rewrites `carton exec prove ...` to `carton exec rtk prove ...`, and the same for the other tools here. `carton exec` stays in front, because it sets `PERL5LIB` to the project's `local/lib/perl5`.
- The TOML filter `src/filters/cpanm.toml` drops per-distribution progress and keeps failures, build log paths and the install count.

## Limits

- `dzil test` and `cover -test` run `make test`, and neither tool can merge the test's STDERR into STDOUT. Their diagnostics print after the summary. Each diagnostic still names its test file and line.

## Measured reduction

These numbers come from the sample in [`scripts/perl-ab/`](../../../scripts/perl-ab/README.md), whose tests fail in nine ways across seven files. In the last A/B check, the rtk reader answered every question that the raw reader answered. A suite with mostly passing files reduces more, because rtk drops passing files completely.

| Scenario | Raw bytes | rtk bytes | Reduction |
|---|---|---|---|
| `cover -report text` | 11193 | 1333 | 89% |
| `cpanm --installdeps .` | 1872 | 282 | 85% |
| `prove -l t-pass` | 325 | 78 | 76% |
| `yath test` | 3480 | 1380 | 61% |
| `prove -l t-bail` (bailout) | 559 | 249 | 56% |
| `cover -test` | 2257 | 1104 | 52% |
| `dzil test` | 3189 | 1563 | 51% |
| `prove -l t` (4 of 7 files fail) | 2384 | 1362 | 43% |
| `perlcritic`, 15 core policies at severity 1 | 3084 | 1827 | 41% |
| `perldoc -f sprintf` | 18063 | 15158 | 17% |

When a run fails, the rtk output of `prove` names the passing files on one line, so that a reader can tell a passing file from a file that did not run. Past 20 files, it prints only the count. A clean run prints one line.

## A/B check

[`scripts/perl-ab/`](../../../scripts/perl-ab/README.md) tests whether the filters keep what an agent needs. Two agents answer the same questions, one from the raw output and one from the rtk output. Run it after you change a filter in this directory.

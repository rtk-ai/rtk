# Captured Go pipe streams

These files are real producer output captured on Linux, not hand-authored events.
The regression tests consume the committed bytes; Cargo/CI does not need Go or
network access to replay them.

- `capture.sh` was run with official Go 1.27.3. It creates a disposable module
  with a passing test, parallel pair, skip, attributes, artifact, failing test,
  logging benchmark, no-test-files package and a deliberately broken package.
- `capture_legacy.sh` was run with official Go 1.24.6. Its normal JSON run covers
  the historical lifecycle events. A separate `-json -v=false` benchmark run
  captures `bench`: with chatty JSON enabled, `b.Log` prints immediately instead
  of producing the buffered `--- BENCH:` report.
- Both scripts retain timestamps, timings and temporary artifact paths exactly
  as emitted. Regeneration changes those values, not the expected summaries.

Run each script with the corresponding Go binary first on `PATH`:

```sh
bash tests/fixtures/go_test_pipe/capture.sh
bash tests/fixtures/go_test_pipe/capture_legacy.sh
```

| File | Producer / purpose |
| --- | --- |
| `core.jsonl` | `go test -json` with parallel tests, skip, empty package and logging benchmark |
| `auxiliary.jsonl` | `go test -json -artifacts` with `t.Attr` and `t.ArtifactDir` |
| `failure.jsonl` | One passing and one failing test with an exact source diagnostic |
| `package_less.jsonl` | Compiled test binary through `go tool test2json -t`, without `-p` |
| `package_less_no_time.jsonl` | Same producer without `-t` or `-p` |
| `mixed_downloads.jsonl` | `go test -json ... 2>&1` with a fresh module cache |
| `build_only.jsonl` | `go build -json` for the broken package, not a test report |
| `legacy_core.jsonl` | Go 1.24.6 lifecycle/parallel/skip/benchmark capture |
| `legacy_bench.jsonl` | Go 1.24.6 non-chatty logging benchmark emitting `Action: bench` |

The modern capture script downloads pinned `golang.org/x/text v0.21.0` and
`golang.org/x/sync v0.10.0` only into disposable capture caches, to obtain genuine
`go: downloading` lines. They are not RTK dependencies. All intentionally failing
producer commands have their exit status checked by the scripts.

The tests add a BOM to the captured core stream and cut its final object in half
without rewriting any event. Separate synthetic malformed/future records test
lossless residual handling. Unknown auxiliary records remain verbatim alongside
the summary; they must not disable reduction of known events. Whole-output
assertions for the core and legacy benchmark captures pin all six lifecycle
handling decisions (`start`, `output`, `skip`, `pause`, `cont`, `bench`).

This does not change the runner or claim a partial stream proves a complete run;
existing incomplete-stream summary semantics remain outside this fix (#4693).

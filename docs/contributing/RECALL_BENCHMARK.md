# Search recall write benchmark

`python3 scripts/benchmark-recall-search.py /path/to/before /path/to/after`
compares release binaries on Linux. Build both with the same Rust toolchain and
release profile. The script creates 100 files containing 80 deterministic matching
lines each, uses `rtk grep --max 2500 -rn needle corpus`, and discards the first
three runs. It interleaves 20 measured runs per binary and recovery configuration
in a fixed random order. Config, recall, tracking, and tee data live in a temporary
directory; telemetry is disabled. No existing RTK data is used.

The four configurations are disabled, tee, SQLite without compression, and SQLite
with gzip. The difference between disabled and raw SQLite includes hashing,
SQLite writes, retention, and hint construction. The gzip/raw difference includes
compression **and the effect of writing a smaller blob**; it is not a pure gzip
microbenchmark. The JSON output includes all timings, mean, standard deviation,
median, output size/hash, and the number of SQLite hints. Capture references are
unique per run, so normalize those identifiers before comparing output hashes.
Use `--max 200` to measure the default display cap, which stores only eight per-file tails and one skipped-files
block, rather than all 100 tails. `--files`, `--lines`, and `--runs` are adjustable.

To time the storage phases separately, run:

```sh
cargo test --release --lib test_benchmark_recall_batch_phases -- --ignored --nocapture
```

This opt-in test measures preparation (hashing, capture IDs, byte caps, line
counting, copying, and optional gzip) separately from the transaction (cached
connection lookup, prepared payload/capture inserts, stats, retention and capture
pruning, survivor lookups, commit). Each codec uses a fresh store so immutable
payload deduplication cannot reuse the other codec. It runs on 100 per-file
payloads, with three warm-ups and 20 measured runs. The preparation
difference between compression on/off isolates gzip from the database work.
There is no timing assertion in the regular test suite.

## Storage contract

Grouped search keeps independently recoverable per-file capture references, payloads,
offsets, and hint positions. Payload preparation happens before the writer
transaction so compression does not hold SQLite's write lock. Eligible payloads,
immutable captures, statistics, and byte-accounting updates commit atomically.
Payload eviction runs once. Orphan capture references are pruned before insertion
(to prevent resurrection) and after eviction. No hint is emitted until commit
succeeds. An insert or eviction failure rolls the eligible batch back and leaves
search output available without recovery links.

If a single search exceeds the payload count/byte limits or the capture-record
limit, those caps still apply. Hints are returned only for captures and payloads
surviving the final retention pass, using each capture's immutable offset and line
count. Oversized entries are skipped without suppressing eligible peers. Existing
payloads retain their original encoding, whose actual stored size must fit the
budget too. An entirely ineligible batch does not commit prune/eviction changes.

Tee mode keeps its file-per-tail behavior. Batching itself adds no schema, default
limit, gzip codec, or connection-cache change beyond its predecessor's contracts.

## Final stacked comparison (2026-10-12)

This comparison isolates batching after the companion changes for bounded stats,
storage budgets, sensitive-output notices, forget cleanup, invocation accounting,
and immutable capture references. Both binaries use those same features. Before
is the exact predecessor `e20801b6ae93a7b47d9d51b93b5f899699353363`; after adds
only the reconciled batch change. Same Rust 1.99.0 release profile, Linux x86_64
shared 9-vCPU host, 30 interleaved measured runs and three warm-ups per case:

| 100 files, 2,500 displayed lines | Before median | After median | Before mean ± SD | After mean ± SD |
| --- | ---: | ---: | ---: | ---: |
| Disabled | 20.48 ms | 19.91 ms | 21.61 ± 3.34 ms | 20.47 ± 1.94 ms |
| Tee | 22.83 ms | 22.06 ms | 23.76 ± 2.78 ms | 23.22 ± 2.85 ms |
| SQLite raw | 107.92 ms | 25.84 ms | 109.86 ± 13.00 ms | 27.16 ± 3.66 ms |
| SQLite gzip | 116.28 ms | 31.57 ms | 114.69 ± 15.21 ms | 32.90 ± 4.63 ms |

Both SQLite cases retain 100 independent per-file capture hints. The gzip median
is about 73% lower against this matched predecessor (raw about 76% lower), with
disabled/tee controls within 1 ms. This larger gain is specific to the integrated
stack: it also removes repeated per-file capture-pruning work. It must not be
presented as a 73% improvement against unmodified develop.

At the default 200-result display cap, nine captures are stored: gzip median
34.31 → 32.40 ms and raw 24.91 → 22.52 ms. A one-file check has no observed
regression: gzip 5.29 → 4.80 ms; disabled control 4.74 → 4.25 ms. The small-case
differences are within the shared host's variability.

Capture metadata accumulates across repeated runs and is bounded by retention.
Each before/after case uses its own isolated store and identical run count. These
results describe this workload and host, not a timing guarantee.

## Original develop comparison (2026-10-12)

Release builds using Rust 1.99.0, Linux x86_64, on the same shared 9-vCPU cloud
host. Before is develop `311ebcbfd8efdb09b03219e7615a153b5b5e3c97`; after is the
initial standalone batch commit `49919c9`, before the companion features were
integrated. These numbers do not describe the final stacked version. Thirty interleaved measured runs per case, after three warm-ups:

| 100 files, 2,500 displayed lines | Before median | After median | Before mean ± SD | After mean ± SD |
| --- | ---: | ---: | ---: | ---: |
| Disabled | 19.85 ms | 20.23 ms | 21.51 ± 4.10 ms | 22.78 ± 10.08 ms |
| Tee | 22.56 ms | 22.43 ms | 23.62 ± 3.35 ms | 23.87 ± 3.78 ms |
| SQLite raw | 27.58 ms | 23.40 ms | 29.59 ± 4.96 ms | 24.12 ± 2.99 ms |
| SQLite gzip | 31.30 ms | 26.54 ms | 33.23 ± 4.23 ms | 28.32 ± 4.43 ms |

Both SQLite cases emitted 100 per-file hints before and after. Their median
end-to-end times fell about 15%. The disabled/tee controls were essentially
unchanged at the median; the shared host introduced considerable noise/outliers.
These are observations for this workload, not portable performance guarantees.

With the default `--max 200`, only nine outputs are stored (eight per-file tails
plus one skipped-files block). SQLite gzip median was 32.71 → 30.48 ms and raw was
22.85 → 21.65 ms. The remaining gzip cost is important on the larger combined
skipped-files block; batching does not remove compression work.

Reproduce both comparisons with:

```sh
python3 scripts/benchmark-recall-search.py /path/to/before /path/to/after --runs 30
python3 scripts/benchmark-recall-search.py /path/to/before /path/to/after --runs 30 --max 200
```

These are fresh measurements of current develop, not a restatement of the older
`ddc21826` results in issue #3946.

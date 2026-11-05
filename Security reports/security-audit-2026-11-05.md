# Security Audit — 2026-11-05

## Summary
- Issues found: 98 (10 dependency advisories, 49 Semgrep findings, 39 gitleaks hits) | Auto-fixed: 7 (dependency advisories) | Unresolved: 91 (3 advisories, 49 Semgrep, 39 gitleaks — see below)
- Status: ISSUES REMAINING
- Scope: upstream merge `origin/develop` (532 new commits, merge commit `ab2196cd`), root `Cargo.lock` and `proxy/Cargo.lock`. Tools run: gitleaks (history + working tree), Semgrep (p/owasp-top-ten, p/secrets), Trivy fs, TruffleHog git, OSV-Scanner. Skipped: Bandit (no `.py` in scope), CodeQL (no workflow run access), mcps-audit / mcp-scan / skillspector (no MCP or AI-skill artifacts in repo; mcp-scan is opt-in), Jev triage (opt-in, not enabled).

## Fixed Issues
| # | Component | Advisory | Change |
|---|-----------|----------|--------|
| 1 | rustls (root `Cargo.lock`) | RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348, RUSTSEC-2026-0349 (GHSA-c8m5-c952-6cm4, GHSA-vp88-798v-h67f, GHSA-h2v8-394m-p7vv, GHSA-w44r-93j7-qv28) | 0.23.45 → 0.23.46 (`cargo update -p rustls --precise 0.23.46`) |
| 2 | rustls-webpki (root) | RUSTSEC-2026-0382 | 0.103.15 → 0.103.16 |
| 3 | webpki-roots (root) | RUSTSEC-2026-0381 | 1.0.6 → 1.0.10 (`cargo update -p webpki-roots@1.0.6 --precise 1.0.10`); 0.26.11 untouched |
| 4 | tokio (`proxy/Cargo.lock`) | RUSTSEC-2026-0386 | 1.53.1 → 1.53.3 |
| 5 | rustls, rustls-webpki, webpki-roots (`proxy/Cargo.lock`) | RUSTSEC-2026-0346/0347/0348/0349, RUSTSEC-2026-0382, RUSTSEC-2026-0381 | 0.23.45 → 0.23.46; 0.103.15 → 0.103.16; 1.0.9 → 1.0.10 |

OSV-Scanner re-run after the fixes: 16 → 3 known vulnerabilities (0 Critical, 0 High, 0 Medium, 1 Low, 2 Unknown). Only `Cargo.lock` lockfile changes; no source code changed.

## Unresolved Issues
1. **lru 0.12.5** — RUSTSEC-2026-0002 / GHSA-rhfx-m35p-ff5j (CVSS 2.7, fix 0.16.3) and RUSTSEC-2026-0253 (fix 0.18.2). Pulled in only by `ratatui 0.29.0` (which pins `lru ^0.12`). Cannot be bumped without upgrading ratatui; that is a dependency migration, not an auto-fix. Needs a manual ratatui upgrade decision.
2. **paste 1.0.15** — RUSTSEC-2024-0436 (unmaintained, no fixed version). No safe replacement applied; needs a maintainer decision on replacing the transitive user.
3. **Semgrep: 49 × `github-actions-mutable-action-tag`** in `.github/workflows/ci.yml`, `release.yml`, `stale.yml`. Actions are referenced by mutable tag instead of commit SHA. Fix is SHA pinning for every `uses:` line, which needs SHA lookups and CI verification. Not auto-applied in an unattended run. No non-GHA Semgrep findings.
4. **gitleaks: 39 hits** (stripe-access-token / generic-api-key rule matches) in the working tree, plus the same entries in git history.
   - `src/cmds/cloud/aws_cmd.rs:1865, 1866, 1910, 2024` — test fixture data (pagination tokens, ARNs with `123` account ids).
   - `tests/guard_integration_test.rs:173, 190` — synthetic test string.
   - `scripts/benchmark/cloud-init.yaml:282` — placeholder credentials in a benchmark provisioning script.
   - `SECURITY.md:137` — code sample in documentation.
   - `Security reports/*.md` — prior audit reports quoting rule patterns.
   - TruffleHog: 4 unverified, **0 verified** live secrets. Classified as false positives; no secret rotation indicated. Not suppressed in config; a human should confirm the `SECURITY.md` sample is a placeholder.
5. Trivy reported the same 9 advisories as OSV (duplicates of items above); no Trivy misconfiguration findings.

## Raw Scanner Output

### gitleaks — history scan (`gitleaks detect`)
```
9:04AM INF 2313 commits scanned.
9:04AM INF scanned ~20231344 bytes (20.23 MB) in 6.25s
9:04AM WRN leaks found: 39
```

### gitleaks — working tree (`--no-git`), rule/file/line
```
stripe-access-token	Security reports/2026-04-16-security-report.md:64
stripe-access-token	Security reports/security-audit-2026-04-18.md:36
stripe-access-token	Security reports/security-audit-2026-04-18.md:39
stripe-access-token	Security reports/security-audit-2026-04-19.md:17
stripe-access-token	SECURITY.md:137
generic-api-key	Security reports/security-audit-2026-05-22.md:36
stripe-access-token	Security reports/security-audit-2026-06-22.md:40
stripe-access-token	Security reports/security-audit-2026-07-01.md:32
generic-api-key	Security reports/security-audit-2026-07-01.md:31
stripe-access-token	Security reports/security-audit-2026-07-07.md:20
generic-api-key	Security reports/security-audit-2026-05-24.md:51
stripe-access-token	Security reports/security-audit-2026-06-08.md:20
generic-api-key	Security reports/security-audit-2026-06-08.md:18
stripe-access-token	Security reports/security-audit-2026-05-24.md:53
stripe-access-token	Security reports/security-audit-2026-06-09.md:48
stripe-access-token	Security reports/security-audit-2026-05-27.md:47
stripe-access-token	Security reports/security-audit-2026-05-27.md:49
stripe-access-token	Security reports/security-audit-2026-06-04.md:21
stripe-access-token	Security reports/security-audit-2026-06-06.md:22
generic-api-key	Security reports/security-audit-2026-06-10.md:59
generic-api-key	Security reports/security-audit-2026-05-29.md:50
generic-api-key	Security reports/security-audit-2026-05-29.md:52
stripe-access-token	Security reports/security-audit-2026-05-29.md:49
generic-api-key	Security reports/security-audit-2026-05-31.md:44
stripe-access-token	Security reports/security-audit-2026-06-17.md:21
stripe-access-token	Security reports/security-audit-2026-06-19.md:53
stripe-access-token	Security reports/security-audit-2026-06-15.md:86
generic-api-key	Security reports/security-audit-2026-06-15.md:83
generic-api-key	scripts/benchmark/cloud-init.yaml:282
generic-api-key	src/cmds/cloud/aws_cmd.rs:1865
generic-api-key	src/cmds/cloud/aws_cmd.rs:1866
generic-api-key	src/cmds/cloud/aws_cmd.rs:1910
generic-api-key	src/cmds/cloud/aws_cmd.rs:1910
generic-api-key	src/cmds/cloud/aws_cmd.rs:2024
stripe-access-token	tests/guard_integration_test.rs:173
stripe-access-token	tests/guard_integration_test.rs:190
```

### OSV-Scanner — before fixes
```
Scanning dir /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4
Starting filesystem walk for root: /
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/proxy/Cargo.lock file and found 319 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/child-b/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/child-a/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multifail-skeleton/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/child-b/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/child-a/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/Cargo.lock file and found 248 packages
End status: 157 dirs visited, 1128 inodes visited, 9 Extract calls, 50.651959ms elapsed, 50.653ms wall time
Warning: enricher transitivedependency/pomxml may be risky when run on untrusted artifacts. Please ensure you trust the source code and artifacts.

Total 9 packages affected by 16 known vulnerabilities (0 Critical, 0 High, 2 Medium, 7 Low, 7 Unknown) from 1 ecosystem.
15 vulnerabilities can be fixed.

+-------------------------------------+------+-----------+---------------+----------+---------------+------------------+
| OSV URL                             | CVSS | ECOSYSTEM | PACKAGE       | VERSION  | FIXED VERSION | SOURCE           |
+-------------------------------------+------+-----------+---------------+----------+---------------+------------------+
| https://osv.dev/RUSTSEC-2026-0002   | 2.7  | crates.io | lru           | 0.12.5   | 0.16.3        | Cargo.lock       |
| https://osv.dev/GHSA-rhfx-m35p-ff5j |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0253   |      | crates.io | lru           | 0.12.5   | 0.18.2        | Cargo.lock       |
| https://osv.dev/RUSTSEC-2024-0436   |      | crates.io | paste         | 1.0.15   | --            | Cargo.lock       |
| https://osv.dev/RUSTSEC-2026-0346   | 4.0  | crates.io | rustls        | 0.23.45  | 0.23.46       | Cargo.lock       |
| https://osv.dev/GHSA-c8m5-c952-6cm4 |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0347   | 2.3  | crates.io | rustls        | 0.23.45  | 0.23.46       | Cargo.lock       |
| https://osv.dev/GHSA-vp88-798v-h67f |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0348   | 3.7  | crates.io | rustls        | 0.23.45  | 0.23.46       | Cargo.lock       |
| https://osv.dev/GHSA-h2v8-394m-p7vv |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0349   | 3.7  | crates.io | rustls        | 0.23.45  | 0.23.46       | Cargo.lock       |
| https://osv.dev/GHSA-w44r-93j7-qv28 |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0382   |      | crates.io | rustls-webpki | 0.103.15 | 0.103.16      | Cargo.lock       |
| https://osv.dev/RUSTSEC-2026-0381   |      | crates.io | webpki-roots  | 1.0.6    | 1.0.10        | Cargo.lock       |
| https://osv.dev/RUSTSEC-2026-0346   | 4.0  | crates.io | rustls        | 0.23.45  | 0.23.46       | proxy/Cargo.lock |
| https://osv.dev/GHSA-c8m5-c952-6cm4 |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0347   | 2.3  | crates.io | rustls        | 0.23.45  | 0.23.46       | proxy/Cargo.lock |
| https://osv.dev/GHSA-vp88-798v-h67f |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0348   | 3.7  | crates.io | rustls        | 0.23.45  | 0.23.46       | proxy/Cargo.lock |
| https://osv.dev/GHSA-h2v8-394m-p7vv |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0349   | 3.7  | crates.io | rustls        | 0.23.45  | 0.23.46       | proxy/Cargo.lock |
| https://osv.dev/GHSA-w44r-93j7-qv28 |      |           |               |          |               |                  |
| https://osv.dev/RUSTSEC-2026-0382   |      | crates.io | rustls-webpki | 0.103.15 | 0.103.16      | proxy/Cargo.lock |
| https://osv.dev/RUSTSEC-2026-0386   |      | crates.io | tokio         | 1.53.1   | 1.53.3        | proxy/Cargo.lock |
| https://osv.dev/RUSTSEC-2026-0381   |      | crates.io | webpki-roots  | 1.0.9    | 1.0.10        | proxy/Cargo.lock |
+-------------------------------------+------+-----------+---------------+----------+---------------+------------------+
```

### OSV-Scanner — after fixes
```
Scanning dir .
Starting filesystem walk for root: /
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/proxy/Cargo.lock file and found 319 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/child-b/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-fail-skeleton/child-a/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multifail-skeleton/pom.xml file and found 1 package
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/child-b/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/tests/fixtures/multi-module-skeleton/child-a/pom.xml file and found 0 packages
Scanned /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/Cargo.lock file and found 248 packages
End status: 157 dirs visited, 1128 inodes visited, 9 Extract calls, 73.468542ms elapsed, 73.469ms wall time
Warning: enricher transitivedependency/pomxml may be risky when run on untrusted artifacts. Please ensure you trust the source code and artifacts.

Total 2 packages affected by 3 known vulnerabilities (0 Critical, 0 High, 0 Medium, 1 Low, 2 Unknown) from 1 ecosystem.
2 vulnerabilities can be fixed.

+-------------------------------------+------+-----------+---------+---------+---------------+------------+
| OSV URL                             | CVSS | ECOSYSTEM | PACKAGE | VERSION | FIXED VERSION | SOURCE     |
+-------------------------------------+------+-----------+---------+---------+---------------+------------+
| https://osv.dev/RUSTSEC-2026-0002   | 2.7  | crates.io | lru     | 0.12.5  | 0.16.3        | Cargo.lock |
| https://osv.dev/GHSA-rhfx-m35p-ff5j |      |           |         |         |               |            |
| https://osv.dev/RUSTSEC-2026-0253   |      | crates.io | lru     | 0.12.5  | 0.18.2        | Cargo.lock |
| https://osv.dev/RUSTSEC-2024-0436   |      | crates.io | paste   | 1.0.15  | --            | Cargo.lock |
+-------------------------------------+------+-----------+---------+---------+---------------+------------+
```

### Trivy fs
```
2026-11-05T09:04:15+07:00	INFO	[vulndb] Need to update DB
2026-11-05T09:04:15+07:00	INFO	[vulndb] Downloading vulnerability DB...
2026-11-05T09:04:15+07:00	INFO	[vulndb] Downloading artifact...	repo="mirror.gcr.io/aquasec/trivy-db:2"
511.25 KiB / 132.22 MiB [>___________________________________________________________] 0.38% ? p/s ?2.92 MiB / 132.22 MiB [->____________________________________________________________] 2.21% ? p/s ?6.17 MiB / 132.22 MiB [-->___________________________________________________________] 4.67% ? p/s ?10.39 MiB / 132.22 MiB [--->____________________________________________] 7.86% 16.47 MiB p/s ETA 7s14.30 MiB / 132.22 MiB [----->_________________________________________] 10.81% 16.47 MiB p/s ETA 7s16.02 MiB / 132.22 MiB [----->_________________________________________] 12.11% 16.47 MiB p/s ETA 7s20.33 MiB / 132.22 MiB [------->_______________________________________] 15.37% 16.48 MiB p/s ETA 6s23.75 MiB / 132.22 MiB [-------->______________________________________] 17.96% 16.48 MiB p/s ETA 6s26.50 MiB / 132.22 MiB [--------->_____________________________________] 20.04% 16.48 MiB p/s ETA 6s29.00 MiB / 132.22 MiB [---------->____________________________________] 21.93% 16.34 MiB p/s ETA 6s34.31 MiB / 132.22 MiB [------------>__________________________________] 25.95% 16.34 MiB p/s ETA 5s37.69 MiB / 132.22 MiB [------------->_________________________________] 28.50% 16.34 MiB p/s ETA 5s41.28 MiB / 132.22 MiB [-------------->________________________________] 31.22% 16.61 MiB p/s ETA 5s44.81 MiB / 132.22 MiB [--------------->_______________________________] 33.89% 16.61 MiB p/s ETA 5s48.34 MiB / 132.22 MiB [----------------->_____________________________] 36.56% 16.61 MiB p/s ETA 5s52.03 MiB / 132.22 MiB [------------------>____________________________] 39.35% 16.70 MiB p/s ETA 4s55.19 MiB / 132.22 MiB [------------------->___________________________] 41.74% 16.70 MiB p/s ETA 4s58.66 MiB / 132.22 MiB [-------------------->__________________________] 44.36% 16.70 MiB p/s ETA 4s61.62 MiB / 132.22 MiB [--------------------->_________________________] 46.61% 16.65 MiB p/s ETA 4s63.78 MiB / 132.22 MiB [---------------------->________________________] 48.24% 16.65 MiB p/s ETA 4s67.25 MiB / 132.22 MiB [----------------------->_______________________] 50.86% 16.65 MiB p/s ETA 3s70.22 MiB / 132.22 MiB [------------------------>______________________] 53.11% 16.50 MiB p/s ETA 3s73.34 MiB / 132.22 MiB [-------------------------->____________________] 55.47% 16.50 MiB p/s ETA 3s76.47 MiB / 132.22 MiB [--------------------------->___________________] 57.83% 16.50 MiB p/s ETA 3s79.75 MiB / 132.22 MiB [---------------------------->__________________] 60.32% 16.46 MiB p/s ETA 3s81.34 MiB / 132.22 MiB [---------------------------->__________________] 61.52% 16.46 MiB p/s ETA 3s83.28 MiB / 132.22 MiB [----------------------------->_________________] 62.99% 16.46 MiB p/s ETA 2s85.47 MiB / 132.22 MiB [------------------------------>________________] 64.64% 16.02 MiB p/s ETA 2s89.22 MiB / 132.22 MiB [------------------------------->_______________] 67.48% 16.02 MiB p/s ETA 2s92.19 MiB / 132.22 MiB [-------------------------------->______________] 69.72% 16.02 MiB p/s ETA 2s95.06 MiB / 132.22 MiB [--------------------------------->_____________] 71.90% 16.01 MiB p/s ETA 2s98.00 MiB / 132.22 MiB [---------------------------------->____________] 74.12% 16.01 MiB p/s ETA 2s101.16 MiB / 132.22 MiB [----------------------------------->__________] 76.51% 16.01 MiB p/s ETA 1s104.28 MiB / 132.22 MiB [------------------------------------>_________] 78.87% 15.97 MiB p/s ETA 1s107.12 MiB / 132.22 MiB [------------------------------------->________] 81.02% 15.97 MiB p/s ETA 1s109.91 MiB / 132.22 MiB [-------------------------------------->_______] 83.12% 15.97 MiB p/s ETA 1s113.22 MiB / 132.22 MiB [--------------------------------------->______] 85.63% 15.90 MiB p/s ETA 1s115.72 MiB / 132.22 MiB [---------------------------------------->_____] 87.52% 15.90 MiB p/s ETA 1s118.94 MiB / 132.22 MiB [----------------------------------------->____] 89.95% 15.90 MiB p/s ETA 0s121.37 MiB / 132.22 MiB [------------------------------------------>___] 91.80% 15.75 MiB p/s ETA 0s124.06 MiB / 132.22 MiB [------------------------------------------->__] 93.83% 15.75 MiB p/s ETA 0s126.75 MiB / 132.22 MiB [-------------------------------------------->_] 95.86% 15.75 MiB p/s ETA 0s130.69 MiB / 132.22 MiB [--------------------------------------------->] 98.84% 15.74 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 15.74 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 15.74 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 14.89 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 14.89 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 14.89 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.93 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.93 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.93 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.03 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.03 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 13.03 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 12.19 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 12.19 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 12.19 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 11.40 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 11.40 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 11.40 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 10.67 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 10.67 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------->] 100.00% 10.67 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.98 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.98 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.98 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.33 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.33 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 9.33 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.73 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.73 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.73 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.17 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.17 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 8.17 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 7.64 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [--------------------------------------------->] 100.00% 7.64 MiB p/s ETA 0s132.22 MiB / 132.22 MiB [-------------------------------------------------] 100.00% 8.64 MiB p/s 15s2026-11-05T09:04:32+07:00	INFO	[vulndb] Artifact successfully downloaded	repo="mirror.gcr.io/aquasec/trivy-db:2"
2026-11-05T09:04:32+07:00	INFO	[vuln] Vulnerability scanning is enabled
2026-11-05T09:04:32+07:00	INFO	[misconfig] Misconfiguration scanning is enabled
2026-11-05T09:04:32+07:00	INFO	[checks-client] Need to update the checks bundle
2026-11-05T09:04:32+07:00	INFO	[checks-client] Downloading the checks bundle...
234.57 KiB / 234.57 KiB [--------------------------------------------------------->] 100.00% ? p/s ?234.57 KiB / 234.57 KiB [-----------------------------------------------] 100.00% 5.68 MiB p/s 200ms2026-11-05T09:04:34+07:00	INFO	[secret] Secret scanning is enabled
2026-11-05T09:04:34+07:00	INFO	[secret] If your scanning is slow, please try '--scanners vuln,misconfig' to disable secret scanning
2026-11-05T09:04:34+07:00	INFO	[secret] Please see https://trivy.dev/docs/v0.76/guide/scanner/secret#recommendation for faster secret detection
2026-11-05T09:04:34+07:00	INFO	Number of language-specific files	num=9
2026-11-05T09:04:34+07:00	INFO	[cargo] Detecting vulnerabilities...
2026-11-05T09:04:34+07:00	INFO	[pom] Detecting vulnerabilities...
2026-11-05T09:04:34+07:00	INFO	Detected config files	num=1

Report Summary

┌───────────────────────────────────────────────────────────┬────────────┬─────────────────┬─────────┬───────────────────┐
│                          Target                           │    Type    │ Vulnerabilities │ Secrets │ Misconfigurations │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ Cargo.lock                                                │   cargo    │        5        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ proxy/Cargo.lock                                          │   cargo    │        4        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-fail-skeleton/child-a/pom.xml │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-fail-skeleton/child-b/pom.xml │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-fail-skeleton/pom.xml         │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-skeleton/child-a/pom.xml      │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-skeleton/child-b/pom.xml      │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multi-module-skeleton/pom.xml              │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/multifail-skeleton/pom.xml                 │    pom     │        0        │    -    │         -         │
├───────────────────────────────────────────────────────────┼────────────┼─────────────────┼─────────┼───────────────────┤
│ tests/fixtures/oc_pods.json                               │ kubernetes │        -        │    -    │         0         │
└───────────────────────────────────────────────────────────┴────────────┴─────────────────┴─────────┴───────────────────┘
Legend:
- '-': Not scanned
- '0': Clean (no security findings detected)


Cargo.lock (cargo)
==================
Total: 5 (UNKNOWN: 0, LOW: 4, MEDIUM: 1, HIGH: 0, CRITICAL: 0)

┌─────────┬─────────────────────┬──────────┬────────┬───────────────────┬───────────────────────┬─────────────────────────────────────────────────────────────┐
│ Library │    Vulnerability    │ Severity │ Status │ Installed Version │     Fixed Version     │                            Title                            │
├─────────┼─────────────────────┼──────────┼────────┼───────────────────┼───────────────────────┼─────────────────────────────────────────────────────────────┤
│ lru     │ GHSA-rhfx-m35p-ff5j │ LOW      │ fixed  │ 0.12.5            │ 0.16.3                │ `IterMut` violates Stacked Borrows by invalidating internal │
│         │                     │          │        │                   │                       │ pointer                                                     │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-rhfx-m35p-ff5j           │
├─────────┼─────────────────────┼──────────┤        ├───────────────────┼───────────────────────┼─────────────────────────────────────────────────────────────┤
│ rustls  │ GHSA-c8m5-c952-6cm4 │ MEDIUM   │        │ 0.23.45           │ 0.23.46, 0.24.0-dev.2 │ Rustls: Session Debug formatting exposes TLS master secrets │
│         │                     │          │        │                   │                       │ and resumption PSKs                                         │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-c8m5-c952-6cm4           │
│         ├─────────────────────┼──────────┤        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-h2v8-394m-p7vv │ LOW      │        │                   │                       │ Rustls: Brotli certificate decompression can allocate an    │
│         │                     │          │        │                   │                       │ attacker-controlled amount of memory up...                  │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-h2v8-394m-p7vv           │
│         ├─────────────────────┤          │        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-vp88-798v-h67f │          │        │                   │                       │ rustls: ECH rejection can expose buffered application data  │
│         │                     │          │        │                   │                       │ to authenticated outer server...                            │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-vp88-798v-h67f           │
│         ├─────────────────────┤          │        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-w44r-93j7-qv28 │          │        │                   │                       │ rustls: An unauthenticated ServerHello poisons the per-name │
│         │                     │          │        │                   │                       │ key-exchange record, so later ClientHellos...               │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-w44r-93j7-qv28           │
└─────────┴─────────────────────┴──────────┴────────┴───────────────────┴───────────────────────┴─────────────────────────────────────────────────────────────┘

proxy/Cargo.lock (cargo)
========================
Total: 4 (UNKNOWN: 0, LOW: 3, MEDIUM: 1, HIGH: 0, CRITICAL: 0)

┌─────────┬─────────────────────┬──────────┬────────┬───────────────────┬───────────────────────┬─────────────────────────────────────────────────────────────┐
│ Library │    Vulnerability    │ Severity │ Status │ Installed Version │     Fixed Version     │                            Title                            │
├─────────┼─────────────────────┼──────────┼────────┼───────────────────┼───────────────────────┼─────────────────────────────────────────────────────────────┤
│ rustls  │ GHSA-c8m5-c952-6cm4 │ MEDIUM   │ fixed  │ 0.23.45           │ 0.23.46, 0.24.0-dev.2 │ Rustls: Session Debug formatting exposes TLS master secrets │
│         │                     │          │        │                   │                       │ and resumption PSKs                                         │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-c8m5-c952-6cm4           │
│         ├─────────────────────┼──────────┤        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-h2v8-394m-p7vv │ LOW      │        │                   │                       │ Rustls: Brotli certificate decompression can allocate an    │
│         │                     │          │        │                   │                       │ attacker-controlled amount of memory up...                  │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-h2v8-394m-p7vv           │
│         ├─────────────────────┤          │        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-vp88-798v-h67f │          │        │                   │                       │ rustls: ECH rejection can expose buffered application data  │
│         │                     │          │        │                   │                       │ to authenticated outer server...                            │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-vp88-798v-h67f           │
│         ├─────────────────────┤          │        │                   │                       ├─────────────────────────────────────────────────────────────┤
│         │ GHSA-w44r-93j7-qv28 │          │        │                   │                       │ rustls: An unauthenticated ServerHello poisons the per-name │
│         │                     │          │        │                   │                       │ key-exchange record, so later ClientHellos...               │
│         │                     │          │        │                   │                       │ https://github.com/advisories/GHSA-w44r-93j7-qv28           │
└─────────┴─────────────────────┴──────────┴────────┴───────────────────┴───────────────────────┴─────────────────────────────────────────────────────────────┘
```

### TruffleHog (git)
```
🐷🔑🐷  TruffleHog. Unearth your secrets. 🐷🔑🐷

2026-11-05T09:04:39+07:00	info-0	trufflehog	running source	{"source_manager_worker_id": "l1C5y", "job_id": 1, "source_id": 1, "source_name": "trufflehog - git", "source_type": "SOURCE_TYPE_GIT", "with_units": true}
Found unverified result 🐷🔑❓
Verification issue: unsupported subprotocol: oracle
Detector Type: JDBC
Decoder Type: PLAIN
Raw result: jdbc:oracle:thin:@localhost:1523:XE`
Commit: fc9ce3dcde5a7dbb52759a9953d4fb3bec553eac
Email: mp3wizard <mp3wizard@gmail.com>
File: Security reports/security-audit-2026-10-31.md
Line: 26
Repository: file:///Users/mp3wizard/Public/Claude%20Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4
Repository_local_path: /var/folders/l4/ncm6zgrd36n9m26knx1f79rr0000gn/T/trufflehog-23666-556870242
Timestamp: 2026-10-31 03:34:03 +0000

Found unverified result 🐷🔑❓
Verification issue: unsupported subprotocol: oracle
(🔍 Using cached verification)
Detector Type: JDBC
Decoder Type: PLAIN
Raw result: jdbc:oracle:thin:@localhost:1523:XE`
Commit: 9053d33f874d3d92493948ee52d22f9aa0eb0503
Email: mp3wizard <mp3wizard@gmail.com>
File: Security reports/security-audit-2026-10-11.md
Line: 26
Repository: file:///Users/mp3wizard/Public/Claude%20Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4
Repository_local_path: /var/folders/l4/ncm6zgrd36n9m26knx1f79rr0000gn/T/trufflehog-23666-556870242
Timestamp: 2026-10-11 15:24:43 +0000

Found unverified result 🐷🔑❓
Verification issue: unsupported subprotocol: oracle
Detector Type: JDBC
Decoder Type: PLAIN
Raw result: jdbc:oracle:thin:@localhost:1523:XE
Commit: 630f0a2800e311d7aee80a9edca22f9f50315f83
Email: s2005 <s2005@users.noreply.github.com>
File: src/filters/liquibase.toml
Line: 73
Repository: file:///Users/mp3wizard/Public/Claude%20Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4
Repository_local_path: /var/folders/l4/ncm6zgrd36n9m26knx1f79rr0000gn/T/trufflehog-23666-556870242
Timestamp: 2026-04-05 13:28:02 +0000

Found unverified result 🐷🔑❓
Verification issue: unsupported subprotocol: oracle
Detector Type: JDBC
Decoder Type: PLAIN
Raw result: jdbc:oracle:thin:@localhost:1523:XE
Commit: 630f0a2800e311d7aee80a9edca22f9f50315f83
Email: s2005 <s2005@users.noreply.github.com>
File: src/filters/liquibase.toml
Line: 78
Repository: file:///Users/mp3wizard/Public/Claude%20Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4
Repository_local_path: /var/folders/l4/ncm6zgrd36n9m26knx1f79rr0000gn/T/trufflehog-23666-556870242
Timestamp: 2026-04-05 13:28:02 +0000

2026-11-05T09:04:42+07:00	info-0	trufflehog	finished scanning	{"chunks": 28062, "bytes": 22214033, "verified_secrets": 0, "unverified_secrets": 4, "scan_duration": "3.626807208s", "trufflehog_version": "3.99.7", "verification_caching": {"Hits":1,"Misses":4,"HitsWasted":0,"AttemptsSaved":1,"VerificationTimeSpentMS":1239}}
```

### Semgrep (p/owasp-top-ten, p/secrets)
```
               
               
┌─────────────┐
│ Scan Status │
└─────────────┘
  Scanning 597 files tracked by git with 595 Code rules:
                                                                                                                        
  Language      Rules   Files          Origin      Rules                                                                
 ─────────────────────────────        ───────────────────                                                               
  <multilang>      41     597          Community     595                                                                
  bash              1      20                                                                                           
  yaml             25      11                                                                                           
  ts               71       9                                                                                           
  json              3       5                                                                                           
  python          147       2                                                                                           
  js               65       1                                                                                           
  ruby             43       1                                                                                           
                                                                                                                        
                    
                    
┌──────────────────┐
│ 49 Code Findings │
└──────────────────┘
                                                                                                                 
    /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/.github/workflows/cd.yml
    ❯❱ yaml.github-actions.security.github-actions-mutable-action-tag.github-actions-mutable-action-tag
          ❰❰ Blocking ❱❱
          GitHub Actions step uses a mutable tag or branch reference. Tags and branch names can be silently   
          repointed by the action owner, enabling supply-chain attacks — as seen in the trivy-action and kics-
          github-action compromises. Pin the reference to a full 40-character commit SHA instead, e.g. `uses: 
          actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608`.                                         
          Details: https://sg.run/2LgAL                                                                       
                                                                                                              
           29┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          157┆ - uses: actions/checkout@v7
                                                                                                                 
    /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/.github/workflows/ci.yml
    ❯❱ yaml.github-actions.security.github-actions-mutable-action-tag.github-actions-mutable-action-tag
          ❰❰ Blocking ❱❱
          GitHub Actions step uses a mutable tag or branch reference. Tags and branch names can be silently   
          repointed by the action owner, enabling supply-chain attacks — as seen in the trivy-action and kics-
          github-action compromises. Pin the reference to a full 40-character commit SHA instead, e.g. `uses: 
          actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608`.                                         
          Details: https://sg.run/2LgAL                                                                       
                                                                                                              
           21┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           35┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           36┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
           46┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           47┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
           50┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
           62┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           63┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
           64┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
           78┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           79┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
           82┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
           96┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
           97┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          100┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
          136┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          139┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          142┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
          162┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          166┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          168┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
          285┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          299┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          301┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          303┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
          316┆ uses: actions/setup-go@v7
            ⋮┆----------------------------------------
          321┆ uses: golangci/golangci-lint-action@v9
            ⋮┆----------------------------------------
          337┆ - uses: actions/checkout@v7
            ⋮┆----------------------------------------
          339┆ - uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          341┆ - uses: Swatinem/rust-cache@v2
            ⋮┆----------------------------------------
          352┆ - uses: actions/checkout@v7
                                                                                                                      
    /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/.github/workflows/release.yml
    ❯❱ yaml.github-actions.security.github-actions-mutable-action-tag.github-actions-mutable-action-tag
          ❰❰ Blocking ❱❱
          GitHub Actions step uses a mutable tag or branch reference. Tags and branch names can be silently   
          repointed by the action owner, enabling supply-chain attacks — as seen in the trivy-action and kics-
          github-action compromises. Pin the reference to a full 40-character commit SHA instead, e.g. `uses: 
          actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608`.                                         
          Details: https://sg.run/2LgAL                                                                       
                                                                                                              
           62┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          110┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          141┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          185┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          195┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          210┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          220┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          223┆ uses: dtolnay/rust-toolchain@stable
            ⋮┆----------------------------------------
          235┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          246┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          265┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          276┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          295┆ uses: actions/upload-artifact@v7
            ⋮┆----------------------------------------
          313┆ uses: actions/checkout@v7
            ⋮┆----------------------------------------
          316┆ uses: actions/download-artifact@v4
                                                                                                                    
    /Users/mp3wizard/Public/Claude Proxy/rtk/.claude/worktrees/fervent-montalcini-2afdb4/.github/workflows/stale.yml
    ❯❱ yaml.github-actions.security.github-actions-mutable-action-tag.github-actions-mutable-action-tag
          ❰❰ Blocking ❱❱
          GitHub Actions step uses a mutable tag or branch reference. Tags and branch names can be silently   
          repointed by the action owner, enabling supply-chain attacks — as seen in the trivy-action and kics-
          github-action compromises. Pin the reference to a full 40-character commit SHA instead, e.g. `uses: 
          actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608`.                                         
          Details: https://sg.run/2LgAL                                                                       
                                                                                                              
           29┆ - uses: actions/stale@v11
                
                
┌──────────────┐
│ Scan Summary │
└──────────────┘
✅ Scan completed successfully.
 • Findings: 49 (49 blocking)
 • Rules run: 331
 • Targets scanned: 597
 • Parsed lines: ~99.9%
 • Scan skipped: 
   ◦ Files larger than  files 0.3 MB: 3
   ◦ Files matching .semgrepignore patterns: 368
 • Scan was limited to files tracked by git
 • For a detailed list of skipped files and lines, run semgrep with the --verbose flag
Ran 331 rules on 597 files: 49 findings.

```

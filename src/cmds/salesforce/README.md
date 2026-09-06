# Salesforce CLI (`sf`)

> Part of [`src/cmds/`](../README.md)

## Scope

Filters high-volume JSON output from the Salesforce CLI for metadata deploy, retrieve, and Apex test runs.

## Commands

| Invocation | Filter |
|------------|--------|
| `rtk sf project deploy start …` | Strips successes, coverage line maps, audit noise |
| `rtk sf project retrieve start …` | Summarizes file inventories; keeps warnings |
| `rtk sf apex run test …` | Failures + low-coverage classes only |
| Other `sf` subcommands | Passthrough (no filtering) |

## Flag injection

For the three filtered commands, RTK injects:

- `--json` when absent (required for structured filtering)
- `--concise` on deploy when absent (CLI-native success trimming)

RTK does **not** inject `--wait`; blocking duration stays under agent control.

## Output modes

- **Default:** compact JSON envelope (valid subset of `sf --json`)
- **`--ultra-compact`:** prose summary (deploy/retrieve) or `TestResult` text (apex tests)

## Example output

This walkthrough uses [`deploy_failed.json`](../../../tests/fixtures/salesforce/deploy_failed.json) — a deploy with a compile error that keeps enough structure to be useful (~94% bash output reduction vs raw `sf --json`).

**Input:** `sf project deploy start --source-dir force-app --json` (162 tokens by RTK's test estimator).

**RTK command:** `rtk sf project deploy start --source-dir force-app` — RTK injects `--json` and `--concise`; same args otherwise.

**Output passed to the model:** minified JSON envelope. RTK emits a single-line minified JSON string; indented here for readability:

```json
{
  "status": 1,
  "result": {
    "details": {
      "componentFailures": [
        {
          "componentType": "ApexClass",
          "fullName": "BadDeploy",
          "problem": "Variable does not exist: y",
          "problemType": "Error",
          "lineNumber": 1,
          "columnNumber": 37,
          "fileName": "classes/BadDeploy.cls"
        }
      ],
      "runTestResult": {
        "numFailures": 0,
        "numTestsRun": 0,
        "totalTime": 0
      }
    },
    "files": [
      {
        "fullName": "BadDeploy",
        "type": "ApexClass",
        "state": "Failed",
        "problemType": "Error",
        "lineNumber": 1,
        "columnNumber": 37,
        "error": "Variable does not exist: y (1:37)"
      }
    ],
    "id": "0AfXXXXXXXXXXXXXXX",
    "numberComponentErrors": 1,
    "status": "Failed",
    "success": false
  },
  "warnings": []
}
```

**Savings:** ~**93.8%** bash output reduction (162 → 10 tokens). Successes, audit fields (`createdBy`, dates, `deployUrl`), and empty coverage arrays are stripped.

## Fixture savings

Token counts use RTK's test estimator (`split_whitespace` word count in `src/core/utils.rs`). Percentages rounded to one decimal. Values are locked by `fixture_savings_table` in `sf_cmd.rs`.

| Fixture | SF command | Filter | Raw tokens | RTK tokens | Savings |
|---------|------------|--------|------------|------------|---------|
| [`deploy_success_verbose.json`](../../../tests/fixtures/salesforce/deploy_success_verbose.json) | `project deploy start` | deploy | 1168 | 1 | ~99.9% |
| [`deploy_success_concise.json`](../../../tests/fixtures/salesforce/deploy_success_concise.json) | `project deploy start --concise` | deploy | 86 | 1 | ~98.8% |
| [`deploy_failed.json`](../../../tests/fixtures/salesforce/deploy_failed.json) | `project deploy start` | deploy | 162 | 10 | ~93.8% |
| [`retrieve_success.json`](../../../tests/fixtures/salesforce/retrieve_success.json) | `project retrieve start` | retrieve | 99 | 1 | ~99.0% |
| [`apex_test_pass_with_coverage.json`](../../../tests/fixtures/salesforce/apex_test_pass_with_coverage.json) | `apex run test` / `apex get test` | apex | 281 | 3 | ~98.9% |
| [`apex_test_failed.json`](../../../tests/fixtures/salesforce/apex_test_failed.json) | `apex run test` (derived) | apex | 289 | 11 | ~96.2% |
| [`apex_test_async.json`](../../../tests/fixtures/salesforce/apex_test_async.json) | `apex run test --wait 0` | passthrough | 11 | 11 | 0% |

- Default filter mode emits **minified JSON**, which often counts as **1 token** when there are no interior spaces.
- [`apex_test_async.json`](../../../tests/fixtures/salesforce/apex_test_async.json) is **passthrough** (only `{ "testRunId": "…" }`); poll with `sf apex get test`.

## Quirks

- Async Apex runs that return only `{ "testRunId": "…" }` are passed through unchanged — poll with `sf apex get test`.
- Parse failures fall back to raw stdout (never block the agent).
- CLI error envelopes without `result` (e.g. `ALREADY_IN_PROCESS`) are passed through on non-zero exit.

## Fixture capture

Refresh fixtures (requires authenticated `sf` with a Salesforce DX project):

```bash
SF_ORG_ALIAS=myorg SFDX_PROJECT=/path/to/sfdx-project ./scripts/capture-sf-fixtures.sh
```

If you are contributing with a new command or flag, remember to anonymize your fixtures before committing.

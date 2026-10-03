#!/usr/bin/env bash
# Capture Salesforce CLI JSON fixtures for RTK filter tests.
#
# Prerequisites:
#   - sf CLI installed and authenticated
#   - A Salesforce DX project with force-app metadata and a target org
#
# Usage (from rtk repo root):
#   SF_ORG_ALIAS=myorg SFDX_PROJECT=/path/to/sfdx-project ./scripts/capture-sf-fixtures.sh
#
# Optional env:
#   SF_APEX_TEST_CLASS  — Apex test class name (default: first *Test.cls in force-app)
#   SF_RETRIEVE_METADATA — Metadata member for retrieve (default: ApexClass:<test class base>)

set -euo pipefail

ORG="${SF_ORG_ALIAS:?Set SF_ORG_ALIAS to your sf org alias}"
SFDX_PROJECT="${SFDX_PROJECT:?Set SFDX_PROJECT to your Salesforce DX project root}"
RTK_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$RTK_ROOT/tests/fixtures/salesforce"
PATH_PREFIX="$SFDX_PROJECT"
PATH_REPLACEMENT="/project/sfdx"

if ! sf org display --target-org "$ORG" >/dev/null 2>&1; then
  echo "error: org alias '$ORG' not available" >&2
  exit 1
fi

if [[ ! -d "$SFDX_PROJECT/force-app" ]]; then
  echo "error: force-app not found under $SFDX_PROJECT" >&2
  exit 1
fi

APEX_TEST="${SF_APEX_TEST_CLASS:-}"
if [[ -z "$APEX_TEST" ]]; then
  APEX_TEST="$(find "$SFDX_PROJECT/force-app" -name '*Test.cls' -print -quit | xargs basename | sed 's/.cls$//')"
fi
if [[ -z "$APEX_TEST" ]]; then
  echo "error: no Apex test class found; set SF_APEX_TEST_CLASS" >&2
  exit 1
fi

RETRIEVE_META="${SF_RETRIEVE_METADATA:-ApexClass:${APEX_TEST%Test}}"
mkdir -p "$FIX"
cd "$SFDX_PROJECT"

echo "Using org=$ORG project=$SFDX_PROJECT apex_test=$APEX_TEST retrieve=$RETRIEVE_META"

echo "Capturing deploy (verbose)..."
sf project deploy start --dry-run --source-dir force-app \
  --target-org "$ORG" --json --wait 15 > "$FIX/deploy_success_verbose.json"

echo "Capturing deploy (concise)..."
sf project deploy start --dry-run --source-dir force-app \
  --target-org "$ORG" --json --concise --wait 15 > "$FIX/deploy_success_concise.json"

echo "Capturing retrieve..."
sf project retrieve start --metadata "$RETRIEVE_META" \
  --target-org "$ORG" --json --wait 10 > "$FIX/retrieve_success.json"

echo "Capturing apex async..."
sf apex run test --tests "$APEX_TEST" \
  --target-org "$ORG" --json --wait 0 > "$FIX/apex_test_async.json"

TEST_RUN_ID="$(python3 -c "import json; print(json.load(open('$FIX/apex_test_async.json'))['result']['testRunId'])")"
echo "Capturing apex results (testRunId=$TEST_RUN_ID)..."
sf apex get test --test-run-id "$TEST_RUN_ID" --target-org "$ORG" \
  --json --code-coverage > "$FIX/apex_test_pass_with_coverage.json"

echo "Capturing deploy failure..."
BAD_DIR="$(mktemp -d)"
trap 'rm -rf "$BAD_DIR"' EXIT
mkdir -p "$BAD_DIR/classes"
cat >"$BAD_DIR/classes/BadDeploy.cls" <<'EOF'
public class BadDeploy { void x() { y = 1; } }
EOF
cat >"$BAD_DIR/classes/BadDeploy.cls-meta.xml" <<'EOF'
<?xml version="1.0"?><ApexClass xmlns="http://soap.sforce.com/2006/04/metadata"><apiVersion>66.0</apiVersion><status>Active</status></ApexClass>
EOF
cat >"$BAD_DIR/package.xml" <<'EOF'
<?xml version="1.0"?><Package xmlns="http://soap.sforce.com/2006/04/metadata"><version>66.0</version></Package>
EOF
sf project deploy start --source-dir "$BAD_DIR" \
  --target-org "$ORG" --json --wait 10 >"$FIX/deploy_failed.json" 2>&1 || true

echo "Sanitizing captured JSON..."
python3 -c "
from pathlib import Path
fix=Path('$FIX'); p='$PATH_PREFIX'; r='$PATH_REPLACEMENT'
for f in fix.glob('*.json'):
    f.write_text(f.read_text().replace(p, r))
"
python3 "$RTK_ROOT/scripts/anonymize-sf-fixtures.py" "$FIX" --derive-failed

echo "Done. Fixtures written to $FIX"

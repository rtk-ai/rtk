"""Build rtk's curated secret ruleset from the gitleaks corpus.

Inclusion criteria (all must hold):
  1. Class A (self-identifying anchor) or Class B (structural). Rules whose only
     keyword is the vendor's own name are rejected -- those match `key = "..."`
     assignments in SOURCE CODE, which is gitleaks' domain. rtk scans stdout.
  2. Plausibly appears in developer CLI output (cloud, VCS/CI, registries,
     AI providers, secret managers, observability, dev SaaS).
  3. At least one anchor >= MIN_ANCHOR_LEN chars. Shorter anchors degrade the
     aho-corasick prefilter that keeps us inside rtk's <10ms budget.
"""
import json
import math
import re
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from sampler import LENGTH_MAX, LENGTH_MID, LENGTH_MIN, sample

MIN_ANCHOR_LEN = 3

# ---------------------------------------------------------------- keeplist
KEEP = {
    "ai": [
        "anthropic-api-key", "anthropic-admin-api-key", "openai-api-key",
        "cohere-api-token", "perplexity-api-key", "huggingface-access-token",
        "huggingface-organization-api-token",
        "aws-amazon-bedrock-api-key-long-lived",
        "aws-amazon-bedrock-api-key-short-lived",
    ],
    "cloud": [
        "aws-access-token", "gcp-api-key", "azure-ad-client-secret",
        "alibaba-access-key-id", "digitalocean-pat", "digitalocean-access-token",
        "digitalocean-refresh-token", "flyio-access-token", "heroku-api-key-v2",
        "cloudflare-origin-ca-key", "clickhouse-cloud-api-secret-key",
        "databricks-api-token", "planetscale-api-token",
        "planetscale-oauth-token", "planetscale-password",
    ],
    "vcs_ci": [
        "github-pat", "github-app-token", "github-oauth",
        "github-fine-grained-pat", "github-refresh-token",
        "gitlab-pat", "gitlab-pat-routable", "gitlab-runner-authentication-token",
        "gitlab-runner-authentication-token-routable", "gitlab-deploy-token",
        "gitlab-cicd-job-token", "gitlab-ptt", "gitlab-rrt",
        "gitlab-oauth-app-secret", "gitlab-session-cookie", "gitlab-feed-token",
        "gitlab-scim-token", "gitlab-kubernetes-agent-token",
        "gitlab-incoming-mail-token", "gitlab-feature-flag-client-token",
        "atlassian-api-token", "sourcegraph-access-token", "harness-api-key",
        "octopus-deploy-api-key", "openshift-user-token", "infracost-api-token",
    ],
    "iac_secrets": [
        "hashicorp-tf-api-token", "hashicorp-tf-password", "vault-service-token",
        "vault-batch-token", "doppler-api-token", "1password-secret-key",
        "1password-service-account-token", "age-secret-key", "pulumi-api-token",
    ],
    "registries": [
        "npm-access-token", "pypi-upload-token", "rubygems-api-token",
        "clojars-api-token", "artifactory-api-key", "artifactory-reference-token",
        "jfrog-api-key", "jfrog-identity-token", "nuget-config-password",
    ],
    "observability": [
        "grafana-api-key", "grafana-cloud-api-token",
        "grafana-service-account-token", "new-relic-user-api-key",
        "new-relic-insert-key", "new-relic-browser-api-token",
        "dynatrace-api-token", "sentry-org-token", "sentry-user-token",
    ],
    "comms": [
        "slack-bot-token", "slack-user-token", "slack-app-token",
        "slack-config-access-token", "slack-config-refresh-token",
        "slack-legacy-bot-token", "slack-legacy-token",
        "slack-legacy-workspace-token", "slack-webhook-url",
        "microsoft-teams-webhook", "sendgrid-api-token", "stripe-access-token",
    ],
    "devtools": [
        "notion-api-token", "linear-api-key", "postman-api-token",
        "prefect-api-token", "shopify-access-token",
        "shopify-custom-access-token", "shopify-private-app-access-token",
        "shopify-shared-secret",
    ],
    # Class B -- structural. Rescued from the vendor-name reject list because
    # they key off output shape, not a vendor word.
    "structural": [
        "private-key", "jwt", "jwt-base64", "kubernetes-secret-yaml",
        "curl-auth-header", "curl-auth-user",
    ],
}

CLASS_B = set(KEEP["structural"])

# Short human labels for the redaction placeholder, e.g.
# [REDACTED:aws_access_key:a1b2]. Falls back to a derived name.
LABEL_OVERRIDES = {
    "aws-access-token": "aws_access_key",
    "gcp-api-key": "gcp_api_key",
    "github-pat": "github_pat",
    "private-key": "private_key",
    "jwt": "jwt",
    "jwt-base64": "jwt",
    "curl-auth-header": "http_auth_header",
    "curl-auth-user": "http_basic_auth",
    "kubernetes-secret-yaml": "k8s_secret",
}


# Patterns narrowed for stream scanning. gitleaks rules assume repo context --
# a bare high-entropy blob sitting near a vendor keyword in a file is
# suspicious. rtk sees `git log` output, where the same blob is a commit SHA.
# Every override here is strictly NARROWER than upstream and states why.
PATTERN_OVERRIDES = {
    # Upstream carries a bare `|[a-fA-F0-9]{40}` branch -- that is every git
    # commit SHA. Caught by the corpus gate against `git log`. Keep only the
    # sgp_-prefixed forms.
    "sourcegraph-access-token":
        r"(?i)\b(sgp_(?:[a-fA-F0-9]{16}|local)_[a-fA-F0-9]{40}"
        r"|sgp_[a-fA-F0-9]{40})\b",
    # Drop the legacy pre-1.10 `s.<24>` form: the `s.` anchor alone fires
    # ~350x per 250KB of ordinary output ("as.", "is.", "this."), which blows
    # the prefilter budget for a token format Vault no longer issues.
    "vault-service-token":
        r"\b(hvs\.[\w-]{90,120})(?:[\x60'\"\s;]|\\[nr]|$)",
}

# gitleaks anchors tuned for scanning source files; a few are poor prefilter
# terms for streaming output. Overrides are strictly narrower.
ANCHOR_OVERRIDES = {
    "sourcegraph-access-token": ["sgp_"],
    "vault-service-token": ["hvs."],
    # Every JWT header begins `{"` -> base64 `eyJ`. gitleaks uses the 2-char
    # `ey`, which fires on ordinary English ("key", "they", "money").
    "jwt": ["eyj"],
}

# Which capture group actually holds the secret.
#
# A redactor replaces the captured group, so a rule whose group 1 is an
# internal fragment would leave most of the token on screen. Two upstream
# patterns have that shape -- gitleaks only needs to *locate* a secret, it
# never has to replace one, so the distinction never mattered there.
SECRET_GROUP_OVERRIDES = {
    # `([a-z0-9]{4}-){3}` makes group 1 the repeated UUID chunk -- 5 chars of
    # a 198-char webhook URL.
    "microsoft-teams-webhook": 0,
    # Group 1 is the named `alg` fragment; the token continues past it.
    "jwt-base64": 0,
}
# Class B rules legitimately capture only the value (`password: <b64>`), so
# that `kind: Secret` stays visible and the output still reads like the real
# command's. Those are correct as-is and are exempt from the coverage check.

# Contextual / structural rules where a realistic fixture beats a synthesised
# one: these should look like output rtk will actually see.
MANUAL_POSITIVES = {
    "hashicorp-tf-password": ['password = "hunter2-abcdef123456"'],
    "curl-auth-header": [
        'curl -H "Authorization: Basic dXNlcjpzdXBlcnNlY3JldHBhc3N3b3Jk"',
    ],
    "curl-auth-user": [
        'curl -u admin:s3cr3tP4ssw0rd https://api.example.com',
        'curl --user deploy:abc123XYZ789 https://registry.internal/v2/',
    ],
    "kubernetes-secret-yaml": [
        'apiVersion: v1\nkind: Secret\nmetadata:\n  name: db-creds\n'
        'type: Opaque\ndata:\n  password: c3VwZXJzZWNyZXQxMjM0NQ==\n',
    ],
}

# Hand-written negatives, emitted alongside the imported ones.
#
# Upstream negatives are filtered by `not rx.search(f)` -- kept only when the
# *pattern* misses them. That cannot express the case below, which the pattern
# matches by design and an allowlist suppresses. Gate 2 evaluates the full
# chain (pattern + entropy + allowlists), so these belong there.
MANUAL_NEGATIVES = {
    # Regression: this fired for real. The templating suppressor was written
    # `{{[^}]+}}`, which the regex crate rejects, and the rejection was
    # swallowed by `Regex::new(p).ok()` -- so a workflow line like this was
    # redacted as a live credential. See escape_literal_braces().
    "curl-auth-user": [
        'curl -u "${{ secrets.REGISTRY_USER }}:${{ secrets.REGISTRY_TOKEN }}"'
        ' https://registry.internal/v2/',
        'curl -u "{{username}}:{{password}}" https://api.example.com',
    ],
}


ASCII_CLASSES = {
    'w': '0-9A-Za-z_',
    'd': '0-9',
    's': ' \\t\\n\\r\\x0b\\x0c',
}


def asciify(pattern: str) -> str:
    """Replace Unicode-aware shorthands with explicit ASCII classes.

    In the regex crate `\\w`, `\\d` and `\\s` are Unicode-aware and compile to
    large UTF-8 automata. Inside a bounded repeat that cost is multiplied: the
    upstream `[\\w-]{138,300}` for a Vault batch token compiles to 14.3MB, and
    pypi's `[\\w-]{50,1000}` to 47.7MB -- both past the crate's 10MB ceiling.
    The same patterns with explicit ASCII classes are 38KB and 140KB.

    Every token these rules match is ASCII (base64/base64url/hex), so this is
    behaviour-preserving on the inputs that matter while removing ~99.7% of the
    compiled size. Verified by the round-trip: positives still match.
    """
    out = []
    i = 0
    in_class = False
    while i < len(pattern):
        c = pattern[i]
        if c == '\\' and i + 1 < len(pattern):
            esc = pattern[i + 1]
            if esc in ASCII_CLASSES:
                body = ASCII_CLASSES[esc]
                out.append(body if in_class else f'[{body}]')
                i += 2
                continue
            out.append(pattern[i:i + 2])
            i += 2
            continue
        if c == '[' and not in_class:
            in_class = True
        elif c == ']' and in_class:
            in_class = False
        out.append(c)
        i += 1
    return ''.join(out)


QUANTIFIER = re.compile(r'\{[0-9]+(?:,[0-9]*)?\}')


def escape_literal_braces(pattern: str) -> str:
    """Escape braces that are not a repetition quantifier.

    Go/RE2 and Python both accept a bare `{` as a literal when it cannot start
    a quantifier. The regex crate does not -- it rejects the pattern outright
    with "repetition quantifier expects a valid decimal". Upstream templating
    suppressors are written `{{[^}]+}}`, so they compile everywhere except the
    engine rtk actually ships.

    That divergence is invisible unless something checks: the pattern is fine
    in the Python loop and fine in gitleaks, and a consumer that compiles
    allowlists leniently just drops the suppressor and starts redacting
    `curl -u "${{ secrets.USER }}:${{ secrets.TOKEN }}"`. Escaping here, and
    gating on it in tests/secret_rules.rs, closes both halves.

    Braces inside a character class are already literal in every engine and are
    left alone.
    """
    out = []
    i = 0
    in_class = False
    while i < len(pattern):
        c = pattern[i]
        if c == '\\' and i + 1 < len(pattern):
            out.append(pattern[i:i + 2])
            i += 2
            continue
        if in_class:
            if c == ']':
                in_class = False
            out.append(c)
            i += 1
            continue
        if c == '[':
            in_class = True
            out.append(c)
            i += 1
            continue
        if c == '{':
            m = QUANTIFIER.match(pattern, i)
            if m:                      # a real repeat: {93}, {1,5}, {138,300}
                out.append(m.group(0))
                i = m.end()
                continue
            out.append(r'\{')
            i += 1
            continue
        if c == '}':                   # unpaired closer: literal, escape it
            out.append(r'\}')
            i += 1
            continue
        out.append(c)
        i += 1
    return ''.join(out)


def rustify(pattern: str) -> str:
    """Convert an upstream RE2 pattern to one the regex crate accepts.

    Two independent divergences, both behaviour-preserving on ASCII input.
    Applied to allowlists as well as patterns -- an allowlist that fails to
    compile fails *open*, in the direction that redacts.
    """
    return escape_literal_braces(asciify(pattern))


def check_bounds(rid: str, pattern: str, rx, report: dict):
    """Exercise every variable-length bound at its low, middle and high end.

    A fixture is generated once, at the minimum of each `{lo,hi}`. That leaves
    the top of every range untested: an upper limit far below what the vendor
    actually issues still reads as passing, because nothing ever asks for a
    longer token. 39 of the rules carry a variable bound.

    Only one sample is committed -- these variants are generated, asserted and
    discarded, so the check costs nothing in fixtures or repo hygiene.
    """
    if not re.search(r'\{\d+,\d*\}', pattern):
        return  # single fixed length: one sample is already exhaustive
    if rid in MANUAL_POSITIVES:
        # Contextual rules the sampler cannot build at any length -- that is
        # why they carry a hand-written fixture. Counted, not silently passed.
        report['bounds_skipped'] = report.get('bounds_skipped', 0) + 1
        return
    for mode in (LENGTH_MIN, LENGTH_MID, LENGTH_MAX):
        try:
            s = sample(pattern, mode).rstrip("`'\";\n\r\t \x0b\x0c")
        except Exception as e:
            report.setdefault('bounds_error', []).append((rid, mode, str(e)))
            continue
        if not (s and rx.search(s)):
            report.setdefault('bounds_miss', []).append((rid, mode, len(s)))
        else:
            report['bounds_ok'] = report.get('bounds_ok', 0) + 1


def extract_allowlists(r: dict):
    """Pull upstream's per-rule false-positive suppressors.

    gitleaks ships allowlists that suppress known-fake values: AWS's documented
    `...EXAMPLE` keys, placeholder credentials like `curl -u user:changeme`,
    `{{templating}}` vars. They are the FP tuning that makes upstream usable, so
    dropping them means shipping a noisier ruleset than the one we imported.

    `paths` allowlists are skipped -- they gate on a file's location, and rtk
    scans a stream that has none.

    Returns (secret_regexes, match_regexes): the first tested against the
    captured secret, the second against the whole match (`regexTarget: match`).
    """
    on_secret, on_match = [], []
    for al in r.get('allowlists', []):
        pats = al.get('regexes')
        if not pats:
            continue  # paths-only allowlist: not applicable to stream scanning
        target = al.get('regexTarget', 'secret')
        (on_match if target == 'match' else on_secret).extend(
            rustify(p) for p in pats)
    return on_secret, on_match


def classify(rid: str, anchors, rx, positive: str) -> str:
    """Class follows detection *shape*, not vendor category.

    Class A -- the anchor lives inside the secret itself (`ghp_`, `AKIA`), so
    the captured group is the whole token.
    Class B -- the anchor is context *around* the secret (`<add key=`,
    `kind: Secret`, `curl`), so the capture is deliberately just the value and
    the surrounding structure stays visible in the output.

    Deriving this from the category was wrong: `nuget-config-password` sits in
    the registries bucket but detects an XML shape, and its capture is the
    password value only -- correct behaviour that the Class A coverage check
    would otherwise flag.
    """
    if rid in CLASS_B:
        return "B"
    m = rx.search(positive)
    if not m:
        return "A"
    idx = SECRET_GROUP_OVERRIDES.get(rid, 1)
    if idx and m.lastindex and m.lastindex >= idx:
        grp = m.group(idx)
    else:
        grp = m.group(0)
    return "A" if any(a in grp.lower() for a in anchors) else "B"


def vendor_name_only(rid: str, keywords) -> bool:
    """True when every keyword is just the vendor's own name.

    Criterion #1: such rules only fire on `key = "..."` assignments in source
    files. rtk reads stdout, where the vendor name and the token do not sit
    next to each other, so the rule can never match -- it is dead weight that
    still costs a prefilter anchor.
    """
    slug = rid.replace('-', '')
    for kw in keywords:
        k = kw.lower()
        if re.search(r'[^a-z0-9]', k) or k not in slug:
            return False
    return True


def anchor_ok(kw: str) -> bool:
    """Prefilter quality gate.

    >=3 chars, or a 2-char term containing a separator (e.g. Azure's `q~`) --
    punctuation makes a short term selective enough to keep the aho-corasick
    pass cheap. Anything shorter fires constantly on prose and would push us
    past rtk's <10ms budget.
    """
    if len(kw) >= MIN_ANCHOR_LEN:
        return True
    return len(kw) == 2 and not kw.isalnum()


HOISTED = []


def pyre(pattern: str):
    """Compile a Go/RE2 pattern under Python's re.

    Go (and Rust's regex crate) allow `(?i)` mid-expression, applying to the
    remainder of the enclosing group; Python 3.11+ rejects it anywhere but the
    start. We hoist it to a leading flag. That is *broader* than the original,
    which only ever drops a negative test case -- it can never manufacture one.
    The Rust test is the authoritative gate for these.
    """
    src = pattern.replace(r'\z', r'\Z')  # Go/Rust end-of-text -> Python
    if '(?i)' in src[1:]:
        HOISTED.append(pattern)
        src = '(?i)' + src.replace('(?i)', '')
    return re.compile(src)


def shannon(s: str) -> float:
    if not s:
        return 0.0
    return -sum((c := s.count(ch) / len(s)) and c * math.log2(c)
                for ch in set(s))


def label_for(rid: str) -> str:
    return LABEL_OVERRIDES.get(rid, rid.replace('-', '_'))


def toml_str(s: str) -> str:
    """Emit a TOML basic string with escapes (safe for regex + newlines)."""
    return '"' + (s.replace('\\', '\\\\').replace('"', '\\"')
                   .replace('\n', '\\n').replace('\r', '\\r')
                   .replace('\t', '\\t')) + '"'


def main():
    rules = {r['id']: r for r in json.load(open('gitleaks_rules.json'))}
    cases = json.load(open('gitleaks_testcases.json'))

    wanted = [(cat, rid) for cat, ids in KEEP.items() for rid in ids]
    out, report = [], {'missing': [], 'weak_anchor': [], 'no_positive': [],
                       'entropy_fail': [], 'ok': 0}

    for cat, rid in wanted:
        r = rules.get(rid)
        if r is None or 'regex' not in r:
            report['missing'].append(rid)
            continue

        kws = r.get('keywords', [])
        # Class B is exempt: its anchor is *supposed* to be surrounding context
        # (`curl`, `kind: secret`), which is precisely what criterion #1 rejects
        # for token rules. Applying the gate to them would drop the structural
        # rules that only rtk needs.
        if (rid not in CLASS_B and rid not in ANCHOR_OVERRIDES
                and vendor_name_only(rid, kws)):
            report.setdefault('vendor_name_only', []).append(
                (rid, ','.join(kws)))
            continue
        anchors = ANCHOR_OVERRIDES.get(
            rid, [k.lower() for k in kws if anchor_ok(k)])
        if not anchors:
            report['weak_anchor'].append(
                (rid, ','.join(r.get('keywords', []))))
            continue

        pattern = rustify(PATTERN_OVERRIDES.get(rid, r['regex']))
        ent_min = r.get('entropy')

        # --- positives: prefer upstream literals, else synthesise ---
        try:
            rx = pyre(pattern)
        except re.error as e:
            report.setdefault('uncompilable', []).append((rid, str(e)))
            continue
        positives = [t for t in MANUAL_POSITIVES.get(rid, []) if rx.search(t)]
        positives += [t for t in cases.get(rid, {}).get('tps', [])
                      if rx.search(t) and t not in positives]
        if not positives:
            # The trailing boundary group `(?:['"\s;]|$)` present on most
            # upstream rules makes the sampler emit a delimiter it did not need.
            # Peel trailing characters back until the sample matches.
            try:
                s = sample(pattern)
                for cut in range(0, 8):
                    cand = (s[:len(s) - cut] if cut else s).rstrip(
                        "`'\";\n\r\t \x0b\x0c")
                    if cand and rx.search(cand):
                        positives = [cand]
                        break
            except Exception:
                pass
        if not positives:
            report['no_positive'].append(rid)
            continue

        # --- entropy floor must be satisfiable by our own positive ---
        if ent_min:
            m = rx.search(positives[0])
            grp = m.group(1) if m.lastindex else m.group(0)
            if shannon(grp) < float(ent_min):
                report['entropy_fail'].append(
                    (rid, round(shannon(grp), 2), ent_min))
                continue

        check_bounds(rid, pattern, rx, report)
        allow_secret, allow_match = extract_allowlists(r)
        negatives = [f for f in cases.get(rid, {}).get('fps', [])
                     if not rx.search(f)]

        out.append({
            'id': rid, 'category': cat,
            'class': classify(rid, anchors, rx, positives[0]),
            'label': label_for(rid),
            'anchors': anchors, 'pattern': pattern,
            'entropy_min': ent_min,
            'narrowed': rid in PATTERN_OVERRIDES,
            'allowlist_secret': allow_secret,
            'allowlist_match': allow_match,
            'secret_group': SECRET_GROUP_OVERRIDES.get(
                rid, r.get('secretGroup')),
            'positives': positives[:3],
            'negatives': negatives[:4] + MANUAL_NEGATIVES.get(rid, []),
            'description': r.get('description', '').strip(),
        })
        report['ok'] += 1

    json.dump(out, open('curated.json', 'w'), indent=1)
    print(f"requested : {len(wanted)}")
    print(f"emitted   : {report['ok']}")
    print(f"bound variants verified (min/mid/max): {report.get('bounds_ok', 0)}"
          f"   [{report.get('bounds_skipped', 0)} rules skipped: hand-written"
          f" fixtures the sampler cannot build]")
    for k in ('missing', 'weak_anchor', 'vendor_name_only', 'no_positive',
              'entropy_fail', 'uncompilable', 'bounds_miss', 'bounds_error'):
        if k not in report:
            continue
        if report[k]:
            print(f"dropped/{k}: {len(report[k])}")
            for x in report[k]:
                print("   ", x)
    tot_pos = sum(len(r['positives']) for r in out)
    tot_neg = sum(len(r['negatives']) for r in out)
    print(f"\npositives : {tot_pos}   negatives(imported): {tot_neg}")
    by_cat = {}
    for r in out:
        by_cat[r['category']] = by_cat.get(r['category'], 0) + 1
    print("by category:", by_cat)
    print(f"patterns needing (?i) hoist for python: {len(HOISTED)}"
          " (validated authoritatively by the Rust test)")


if __name__ == '__main__':
    sys.exit(main())

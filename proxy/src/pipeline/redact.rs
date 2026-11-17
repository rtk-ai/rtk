//! Secret-redaction transform: scrubs obvious credential patterns from the
//! request body before it leaves the building.
//!
//! This is a defensive, best-effort stage — it is NOT a substitute for a
//! dedicated secret scanner. Agents extending this should keep patterns conservative to avoid
//! corrupting legitimate prompts.

use crate::error::AppResult;
use crate::model::InterceptedRequest;
use crate::pipeline::Transform;
use once_cell::sync::Lazy;
use regex::{Regex, RegexSet};
use std::borrow::Cow;

/// High-confidence secret patterns (provider keys), replaced with a marker.
static PATTERNS: Lazy<Vec<(Regex, &'static str)>> = Lazy::new(|| {
    vec![
        (
            Regex::new(r"sk-ant-[A-Za-z0-9_\-]{20,}").unwrap(),
            "[REDACTED_ANTHROPIC_KEY]",
        ),
        (
            Regex::new(r"sk-[A-Za-z0-9]{32,}").unwrap(),
            "[REDACTED_OPENAI_KEY]",
        ),
        (
            Regex::new(r"ghp_[A-Za-z0-9]{36}").unwrap(),
            "[REDACTED_GITHUB_PAT]",
        ),
        (
            Regex::new(r"AKIA[0-9A-Z]{16}").unwrap(),
            "[REDACTED_AWS_KEY]",
        ),
    ]
});

/// All patterns combined for a single-pass "does anything match?" gate.
static PATTERN_SET: Lazy<RegexSet> =
    Lazy::new(|| RegexSet::new(PATTERNS.iter().map(|(re, _)| re.as_str())).unwrap());

pub struct RedactTransform;

impl RedactTransform {
    /// Redact known secret patterns in `text`. Returns `(redacted, hits)`.
    ///
    /// Fast path: one combined `RegexSet` scan; when nothing matches (the common
    /// case) the input is returned borrowed, with **zero allocation**. Only on a
    /// hit is the string cloned and the per-pattern replacement run.
    pub fn scrub(text: &str) -> (Cow<'_, str>, usize) {
        if !PATTERN_SET.is_match(text) {
            return (Cow::Borrowed(text), 0);
        }
        let mut out = text.to_string();
        let mut hits = 0usize;
        for (re, replacement) in PATTERNS.iter() {
            let count = re.find_iter(&out).count();
            if count > 0 {
                hits += count;
                out = re.replace_all(&out, *replacement).into_owned();
            }
        }
        (Cow::Owned(out), hits)
    }
}

#[async_trait::async_trait]
impl Transform for RedactTransform {
    fn name(&self) -> &'static str {
        "redact"
    }

    async fn on_request(&self, req: &mut InterceptedRequest) -> AppResult<()> {
        let mut hits = 0usize;
        redact_json(&mut req.body, &mut hits);
        if hits > 0 {
            tracing::info!(hits, "redacted secrets from request body");
        }
        Ok(())
    }
}

fn redact_json(value: &mut serde_json::Value, hits: &mut usize) {
    match value {
        serde_json::Value::String(s) => {
            // Skip large blobs (base64 images/PDFs): real credentials are short,
            // and scanning binary base64 risks a false-positive match (e.g. AKIA)
            // silently corrupting an image. 2 KB is far above any real secret.
            const MAX_REDACT_SCAN: usize = 2048;
            if s.len() <= MAX_REDACT_SCAN {
                let (scrubbed, n) = RedactTransform::scrub(s);
                if n > 0 {
                    *s = scrubbed.into_owned();
                    *hits += n;
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                redact_json(v, hits);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                redact_json(v, hits);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Provider;

    #[test]
    fn scrubs_anthropic_key() {
        let (out, hits) = RedactTransform::scrub("key=sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA done");
        assert_eq!(hits, 1);
        assert!(out.contains("[REDACTED_ANTHROPIC_KEY]"));
        assert!(!out.contains("sk-ant-"));
    }

    #[test]
    fn scrubs_openai_key() {
        let key = format!("sk-{}", "a".repeat(40));
        let input = format!("token {key}");
        let (out, hits) = RedactTransform::scrub(&input);
        assert_eq!(hits, 1);
        assert!(out.contains("[REDACTED_OPENAI_KEY]"));
    }

    #[test]
    fn scrubs_github_pat() {
        let pat = format!("ghp_{}", "A".repeat(36));
        let (out, hits) = RedactTransform::scrub(&pat);
        assert_eq!(hits, 1);
        assert_eq!(&*out, "[REDACTED_GITHUB_PAT]");
    }

    #[test]
    fn scrubs_aws_key() {
        let (out, hits) = RedactTransform::scrub("AKIAIOSFODNN7EXAMPLE rest");
        assert_eq!(hits, 1);
        assert!(out.contains("[REDACTED_AWS_KEY]"));
    }

    #[test]
    fn leaves_clean_text_untouched() {
        let clean = "the quick brown fox jumps over the lazy dog";
        let (out, hits) = RedactTransform::scrub(clean);
        assert_eq!(hits, 0);
        assert_eq!(&*out, clean);
    }

    #[test]
    fn multiple_secrets_counted() {
        let aws = "AKIAIOSFODNN7EXAMPLE";
        let text = format!("{aws} and {aws}");
        let (_, hits) = RedactTransform::scrub(&text);
        assert_eq!(hits, 2);
    }

    #[tokio::test]
    async fn on_request_walks_nested_json() {
        let body = serde_json::json!({
            "model": "claude",
            "messages": [
                { "content": "my key is sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA please keep it" }
            ],
            "meta": { "note": "AKIAIOSFODNN7EXAMPLE" }
        });
        let mut req = crate::model::InterceptedRequest {
            provider: Provider::Anthropic,
            path: "/v1/messages".to_string(),
            model: "claude".to_string(),
            stream: false,
            client: None,
            project: None,
            session: None,
            api_key_hash: None,
            tokens_saved: 0,
            saved_by_stage: Vec::new(),
            routed_from: None,
            request_class: None,
            user_id: None,
            ccr: Default::default(),
            ccr_retrieve_calls: 0,
            pipeline_ms: 0,
            budget_reserved: 0.0,
            team_budget_reserved: 0.0,
            team_budget_reserved_at: None,
            body,
        };
        RedactTransform.on_request(&mut req).await.unwrap();
        let serialized = serde_json::to_string(&req.body).unwrap();
        assert!(serialized.contains("[REDACTED_ANTHROPIC_KEY]"));
        assert!(serialized.contains("[REDACTED_AWS_KEY]"));
        assert!(!serialized.contains("sk-ant-"));
        assert!(!serialized.contains("AKIA"));
    }
    #[test]
    fn large_base64_blob_is_not_scanned() {
        // A base64 image blob containing an AKIA-looking run must NOT be
        // redacted (would silently corrupt the image). Short secrets still are.
        let blob = "AKIAIOSFODNN7EXAMPLE".to_string() + &"QWxhZGRpbjpvcGVuIHNlc2FtZQ".repeat(200);
        assert!(blob.len() > 2048);
        let mut v = serde_json::json!({ "messages": [ { "role": "user", "content": [
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": blob } }
        ] } ] });
        let mut hits = 0usize;
        super::redact_json(&mut v, &mut hits);
        assert_eq!(hits, 0, "large blob skipped");
        assert!(
            v["messages"][0]["content"][0]["source"]["data"]
                .as_str()
                .unwrap()
                .starts_with("AKIA"),
            "blob intact"
        );
        // A short field with the same secret is still redacted.
        let mut v2 = serde_json::json!({ "text": "creds AKIAIOSFODNN7EXAMPLE here" });
        let mut h2 = 0usize;
        super::redact_json(&mut v2, &mut h2);
        assert_eq!(h2, 1);
    }
}

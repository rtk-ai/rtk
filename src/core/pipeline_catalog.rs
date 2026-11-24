//! The reviewed-pipeline catalog published at rtk-ai.app/pipelines: fetched
//! live with a short timeout. When the site cannot be reached, the newer of
//! two copies is used: the last one this machine fetched, or the one shipped
//! in the binary (`pipeline_catalog.json`, refreshed from the live index
//! before a release). Revoked entries are dropped, and only entries pinned to
//! a GitHub commit are kept, so init never installs an unpinned source.

use crate::core::{http, user_dirs};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

pub const CATALOG_URL: &str = "https://www.rtk-ai.app/pipelines/index.json";
pub const CATALOG_PAGE: &str = "https://www.rtk-ai.app/pipelines/";
pub const DEFAULT_PIPELINE: &str = "default-bash-compress";
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_FILE: &str = "pipeline-catalog.json";
const SHIPPED: &str = include_str!("pipeline_catalog.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub result: Option<EntryResult>,
    #[serde(default)]
    pub requires: Vec<Requirement>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub install: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntryResult {
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Requirement {
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct Index {
    #[serde(default)]
    generated: String,
    #[serde(default)]
    pipelines: Vec<Entry>,
    #[serde(default)]
    revoked: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Live,
    Cached,
    Shipped,
    Missing,
}

pub struct Catalog {
    pub entries: Vec<Entry>,
    pub origin: Origin,
}

impl Entry {
    pub fn source(&self) -> Option<&str> {
        let source = self
            .install
            .trim()
            .strip_prefix("rtk pipeline add ")?
            .trim();
        if source.contains(char::is_whitespace) {
            return None;
        }
        let parsed = crate::core::pipeline_remote::parse_github_source(source)?;
        let pinned = parsed
            .reference
            .as_deref()
            .is_some_and(|r| r.len() == 40 && r.chars().all(|c| c.is_ascii_hexdigit()));
        let same_repo = format!("{}/{}", parsed.owner, parsed.repo) == self.id;
        (pinned && same_repo).then_some(source)
    }

    pub fn canonical(&self) -> String {
        format!("github:{}", self.id)
    }

    pub fn is_default(&self) -> bool {
        self.name == DEFAULT_PIPELINE
    }

    pub fn needs(&self) -> String {
        if self.requires.is_empty() {
            return "needs nothing".to_string();
        }
        let names: Vec<&str> = self.requires.iter().map(|r| r.name.as_str()).collect();
        format!("needs {}", names.join(", "))
    }

    pub fn saves(&self) -> Option<String> {
        let result = self.result.as_ref()?;
        if result.value.is_empty() {
            return None;
        }
        Some(format!("{} cost", result.value))
    }
}

pub fn load() -> Catalog {
    if let Ok(text) = fetch()
        && let Ok((_, entries)) = parse(&text)
    {
        save(&text);
        return Catalog {
            entries,
            origin: Origin::Live,
        };
    }
    let saved = cache_path().and_then(|path| std::fs::read_to_string(path).ok());
    offline(saved.as_deref(), SHIPPED)
}

fn offline(saved: Option<&str>, shipped: &str) -> Catalog {
    let saved = saved.and_then(|text| parse(text).ok());
    let shipped = parse(shipped).ok();
    let (entries, origin) = match (saved, shipped) {
        (Some(saved), Some(shipped)) if shipped.0 > saved.0 => (shipped.1, Origin::Shipped),
        (Some(saved), _) => (saved.1, Origin::Cached),
        (None, Some(shipped)) => (shipped.1, Origin::Shipped),
        (None, None) => (Vec::new(), Origin::Missing),
    };
    Catalog { entries, origin }
}

pub fn find<'a>(entries: &'a [Entry], name: &str) -> Option<&'a Entry> {
    entries.iter().find(|e| e.name == name || e.id == name)
}

fn fetch() -> anyhow::Result<String> {
    let mut response = http::get_within(CATALOG_URL, FETCH_TIMEOUT)?;
    http::read_text_lossy(&mut response, http::MAX_BODY)
}

fn cache_path() -> Option<PathBuf> {
    user_dirs::data().map(|dir| dir.join(CACHE_FILE))
}

fn save(text: &str) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

fn parse(text: &str) -> anyhow::Result<(String, Vec<Entry>)> {
    let index: Index = serde_json::from_str(text)?;
    let revoked = index.revoked;
    let entries = index
        .pipelines
        .into_iter()
        .filter(|e| e.source().is_some() && !is_revoked(e, &revoked))
        .collect();
    Ok((index.generated, entries))
}

fn is_revoked(entry: &Entry, revoked: &[serde_json::Value]) -> bool {
    let pinned = entry
        .sha
        .as_deref()
        .map(|sha| format!("{}@{sha}", entry.id));
    revoked.iter().any(|r| match r {
        serde_json::Value::String(s) => {
            *s == entry.id || *s == entry.name || pinned.as_deref() == Some(s.as_str())
        }
        serde_json::Value::Object(o) => {
            o.get("id").and_then(|v| v.as_str()) == Some(entry.id.as_str())
                && o.get("sha")
                    .and_then(|v| v.as_str())
                    .is_none_or(|sha| Some(sha) == entry.sha.as_deref())
        }
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_entries(text: &str) -> anyhow::Result<Vec<Entry>> {
        parse(text).map(|(_, entries)| entries)
    }

    const SHA: &str = "ef1b85b59cb670b70a40a7cb0dbb8e699bb6ba9f";

    fn index(revoked: &str) -> String {
        format!(
            r#"{{"generated":"x","pipelines":[
              {{"id":"aeppling/rtk-pipeline-default-bash-compress","name":"default-bash-compress",
                "summary":"default","result":{{"value":"−6.2%","unit":"per trial"}},"requires":[],
                "sha":"{SHA}","install":"rtk pipeline add github:aeppling/rtk-pipeline-default-bash-compress@{SHA}"}},
              {{"id":"aeppling/rtk-pipeline-laya-delegate","name":"laya-delegate","summary":"delegate",
                "requires":[{{"kind":"local-service","name":"Laya","env":"RTK_LAYA_TOKEN"}},{{"kind":"api-key","name":"Anthropic API key"}}],
                "sha":"{SHA}","install":"rtk pipeline add github:aeppling/rtk-pipeline-laya-delegate@{SHA}"}}
            ],"revoked":[{revoked}]}}"#
        )
    }

    #[test]
    fn parses_entries_with_their_pinned_source() {
        let entries = parse_entries(&index("")).expect("valid index");
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].source(),
            Some(format!("github:aeppling/rtk-pipeline-default-bash-compress@{SHA}").as_str())
        );
        assert!(entries[0].is_default());
        assert_eq!(entries[0].needs(), "needs nothing");
        assert_eq!(entries[0].saves().as_deref(), Some("−6.2% cost"));
        assert_eq!(entries[1].needs(), "needs Laya, Anthropic API key");
        assert_eq!(entries[1].saves(), None);
    }

    #[test]
    fn revoked_entries_are_dropped_by_id_pin_or_object() {
        let by_id =
            parse_entries(&index(r#""aeppling/rtk-pipeline-laya-delegate""#)).expect("index");
        assert_eq!(by_id.len(), 1);
        let by_pin = parse_entries(&index(&format!(
            r#""aeppling/rtk-pipeline-laya-delegate@{SHA}""#
        )))
        .expect("index");
        assert_eq!(by_pin.len(), 1);
        let other_pin = parse_entries(&index(
            r#"{"id":"aeppling/rtk-pipeline-laya-delegate","sha":"0000000000000000000000000000000000000000"}"#,
        ))
        .expect("index");
        assert_eq!(other_pin.len(), 2);
    }

    #[test]
    fn unpinned_or_mismatched_sources_are_refused() {
        let text = r#"{"pipelines":[
          {"id":"a/rtk-pipeline-x","name":"x","install":"rtk pipeline add github:a/rtk-pipeline-x"},
          {"id":"a/rtk-pipeline-y","name":"y","install":"rtk pipeline add github:evil/rtk-pipeline-y@ef1b85b59cb670b70a40a7cb0dbb8e699bb6ba9f"},
          {"id":"a/rtk-pipeline-z","name":"z","install":"rtk pipeline add https://example.com/z.toml"},
          {"id":"a/rtk-pipeline-w","name":"w","install":"rtk pipeline add github:a/rtk-pipeline-w@ef1b85b59cb670b70a40a7cb0dbb8e699bb6ba9f --force"}
        ]}"#;
        assert!(parse_entries(text).expect("index").is_empty());
    }

    #[test]
    fn shipped_copy_is_valid_and_offers_the_default() {
        let entries = parse_entries(SHIPPED).expect("shipped catalog parses");
        assert!(entries.iter().any(Entry::is_default));
        assert!(entries.iter().all(|e| e.source().is_some()));
    }

    #[test]
    fn offline_uses_the_newer_of_the_saved_and_shipped_copies() {
        let dated = |date: &str, name: &str| {
            format!(
                r#"{{"generated":"{date}","pipelines":[{{"id":"a/rtk-pipeline-{name}","name":"{name}",
                  "install":"rtk pipeline add github:a/rtk-pipeline-{name}@{SHA}"}}]}}"#
            )
        };
        let old = dated("2026-01-01T00:00:00Z", "old");
        let new = dated("2026-06-01T00:00:00Z", "new");
        let names = |c: &Catalog| c.entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>();

        let saved_newer = offline(Some(&new), &old);
        assert_eq!(saved_newer.origin, Origin::Cached);
        assert_eq!(names(&saved_newer), ["new"]);

        let shipped_newer = offline(Some(&old), &new);
        assert_eq!(shipped_newer.origin, Origin::Shipped);
        assert_eq!(names(&shipped_newer), ["new"]);

        let nothing_saved = offline(None, &old);
        assert_eq!(nothing_saved.origin, Origin::Shipped);

        let corrupt_saved = offline(Some("{not json"), &old);
        assert_eq!(corrupt_saved.origin, Origin::Shipped);

        assert_eq!(offline(None, "{not json").origin, Origin::Missing);
    }

    #[test]
    fn find_matches_name_or_id() {
        let entries = parse_entries(&index("")).expect("index");
        assert_eq!(
            find(&entries, "laya-delegate").map(|e| e.name.as_str()),
            Some("laya-delegate")
        );
        assert_eq!(
            find(&entries, "aeppling/rtk-pipeline-laya-delegate").map(|e| e.name.as_str()),
            Some("laya-delegate")
        );
        assert!(find(&entries, "nope").is_none());
    }
}

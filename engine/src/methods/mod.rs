pub mod ansi;
pub mod base64_elide;
pub mod cap;
pub mod dedup;
pub mod grep_group;
pub mod lockfile;
pub mod md_slim;
pub mod minify_json;
pub mod progress;
pub mod rle;
pub mod shape;
pub mod strip_line_numbers;
pub mod toon;
pub mod truncate;
pub mod web_md;

use crate::block::ContentFormat;
use crate::method::Method;

pub static ANSI: ansi::Ansi = ansi::Ansi;
pub static BASE64_ELIDE: base64_elide::Base64Elide = base64_elide::Base64Elide;
pub static TOON: toon::Toon = toon::Toon;
pub static MINIFY_JSON: minify_json::MinifyJson = minify_json::MinifyJson;
pub static WEB_MD: web_md::WebMd = web_md::WebMd;
pub static MD_SLIM: md_slim::MdSlim = md_slim::MdSlim;
pub static TRUNCATE: truncate::Truncate = truncate::Truncate;
pub static GREP_GROUP: grep_group::GrepGroup = grep_group::GrepGroup;
pub static LOCKFILE: lockfile::Lockfile = lockfile::Lockfile;
pub static PROGRESS: progress::Progress = progress::Progress;
pub static DEDUP: dedup::Dedup = dedup::Dedup;
pub static CAP: cap::Cap = cap::Cap;
pub static SHAPE: shape::ShapeMethod = shape::ShapeMethod;
pub static RLE: rle::Rle = rle::Rle;
pub static STRIP_LINE_NUMBERS: strip_line_numbers::StripLineNumbers =
    strip_line_numbers::StripLineNumbers;

// Built-in chains are conversion-only: no truncation on
// the posthook path — content is reformatted, never dropped. `truncate`
// stays available as a reserved method.
static JSON_CHAIN: [&dyn Method; 2] = [&BASE64_ELIDE, &TOON];
// `toon` after `web-md`: fires only when the response body parses as JSON
// (raw JSON API responses) and short-circuits on conversion, so `md-slim`
// only ever sees markdown/plain text.
static WEB_CHAIN: [&dyn Method; 5] = [&ANSI, &BASE64_ELIDE, &WEB_MD, &TOON, &MD_SLIM];
// No `ansi`/`base64-elide` here: bytes in a Grep match are genuine file
// content (the host never colors Grep output) — rewriting them would
// falsify the file.
static MATCHES_CHAIN: [&dyn Method; 1] = [&GREP_GROUP];
static LOCKFILE_CHAIN: [&dyn Method; 1] = [&LOCKFILE];
// Bash generic floor: objective byte-level transforms only, each lossless or
// explicitly marked and recoverable via the recall file. `cap` is tail-biased
// (bash errors live at the end) — the one deliberately lossy step, requested
// for runaway outputs.
static TERM_CHAIN: [&dyn Method; 5] = [&ANSI, &PROGRESS, &DEDUP, &BASE64_ELIDE, &CAP];

pub fn chain_for(format: ContentFormat) -> &'static [&'static dyn Method] {
    match format {
        ContentFormat::Json => &JSON_CHAIN,
        ContentFormat::Web => &WEB_CHAIN,
        ContentFormat::Matches => &MATCHES_CHAIN,
        ContentFormat::Lockfile => &LOCKFILE_CHAIN,
        ContentFormat::Term => &TERM_CHAIN,
    }
}

pub fn named_chain(name: &str) -> Option<&'static [&'static dyn Method]> {
    match name {
        "term" => Some(&TERM_CHAIN),
        "json" => Some(&JSON_CHAIN),
        "web" => Some(&WEB_CHAIN),
        "matches" => Some(&MATCHES_CHAIN),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::run_chain;
    use crate::method::MethodCtx;
    use crate::testutil;

    #[test]
    fn test_chain_layer_names_are_frozen_vocabulary() {
        let names: Vec<&str> = chain_for(ContentFormat::Web)
            .iter()
            .map(|l| l.name())
            .collect();
        assert_eq!(
            names,
            vec!["ansi", "base64-elide", "web-md", "toon", "md-slim"]
        );

        let names: Vec<&str> = chain_for(ContentFormat::Matches)
            .iter()
            .map(|l| l.name())
            .collect();
        assert_eq!(names, vec!["grep-group"]);

        let names: Vec<&str> = chain_for(ContentFormat::Json)
            .iter()
            .map(|l| l.name())
            .collect();
        assert_eq!(names, vec!["base64-elide", "toon"]);

        let names: Vec<&str> = chain_for(ContentFormat::Lockfile)
            .iter()
            .map(|l| l.name())
            .collect();
        assert_eq!(names, vec!["lockfile"]);

        let names: Vec<&str> = chain_for(ContentFormat::Term)
            .iter()
            .map(|l| l.name())
            .collect();
        assert_eq!(
            names,
            vec!["ansi", "progress", "dedup", "base64-elide", "cap"]
        );
    }

    #[test]
    fn test_web_chain_json_conversion_never_line_capped() {
        // Nested JSON → toon falls back to minify → ONE long line. toon must
        // ShortCircuit so truncate's 500-char line cap can't chop the data.
        let ctx = MethodCtx {
            source: "https://api.example.com/big",
        };
        let items: Vec<String> = (0..200)
            .map(|i| format!("{{\"k{i}\": {{\"nested\": \"value_{i}\"}}}}"))
            .collect();
        let input = format!("[\n  {}\n]", items.join(",\n  "));
        let block = testutil::block(&input, ContentFormat::Web);
        let out = run_chain(chain_for(ContentFormat::Web), &block, &ctx);
        assert!(out.len() > 2000, "minified one-liner must survive whole");
        assert!(!out.ends_with("..."), "must not be line-capped");
        serde_json::from_str::<serde_json::Value>(&out).expect("still valid JSON");
    }

    #[test]
    fn test_web_chain_toon_encodes_raw_json_response() {
        // A WebFetch/WebSearch body that is raw JSON (API endpoint): web-md
        // skips it (not HTML), toon picks it up.
        let ctx = MethodCtx {
            source: "https://api.example.com/users",
        };
        let input = r#"[
  {"id": 1, "name": "a"},
  {"id": 2, "name": "b"},
  {"id": 3, "name": "c"}
]"#;
        let block = testutil::block(input, ContentFormat::Web);
        let out = run_chain(chain_for(ContentFormat::Web), &block, &ctx);
        assert!(
            out.starts_with("[3]{id,name}:"),
            "raw JSON over web must TOON-encode, got: {out}"
        );
    }

    #[test]
    fn test_web_chain_markdown_untouched_by_toon() {
        let ctx = MethodCtx {
            source: "https://example.com",
        };
        let input = "# Title\n\nSome *markdown* text.\n";
        let block = testutil::block(input, ContentFormat::Web);
        assert_eq!(
            run_chain(chain_for(ContentFormat::Web), &block, &ctx),
            input
        );
    }

    #[test]
    fn test_run_chain_applies_layers_in_order() {
        let ctx = MethodCtx { source: "test" };
        let input = "\x1b[31mhello\x1b[0m";
        let block = testutil::block(input, ContentFormat::Web);
        let out = run_chain(&[&ANSI], &block, &ctx);
        assert_eq!(out, "hello");
    }

    #[test]
    fn test_run_chain_empty_input() {
        let ctx = MethodCtx {
            source: "test.json",
        };
        let block = testutil::block("", ContentFormat::Json);
        let out = run_chain(chain_for(ContentFormat::Json), &block, &ctx);
        assert_eq!(out, "");
    }

    /// These methods lose data (CR overwrites, >i64 ints,
    /// duplicate keys, indentation) and say so. Pin the labels so they
    /// stay accurate, and recall stays wired for them.
    #[test]
    fn destructive_methods_declare_their_loss() {
        use crate::method::Recovery;
        let cases: &[&dyn Method] = &[&PROGRESS, &TOON, &MINIFY_JSON, &GREP_GROUP, &RLE];
        for method in cases {
            let info = method.info();
            assert!(
                info.lossy,
                "{} destroys data and must say so",
                method.name()
            );
            assert_eq!(
                info.recovery,
                Recovery::Recall,
                "{}: a lossy label without recovery just loses data politely",
                method.name()
            );
        }
    }

    #[test]
    fn lossless_methods_are_idempotent() {
        let cases: Vec<(&dyn Method, ContentFormat, &str)> = vec![
            (&ANSI, ContentFormat::Term, "\x1b[32mok\x1b[0m done"),
            (&MINIFY_JSON, ContentFormat::Json, "{\n  \"a\": 1\n}"),
            (
                &TOON,
                ContentFormat::Json,
                r#"[{"id":1,"n":"a"},{"id":2,"n":"b"}]"#,
            ),
            (
                &GREP_GROUP,
                ContentFormat::Matches,
                "src/a.rs:1:fn a() {}\nsrc/a.rs:9:fn b() {}\nsrc/c.rs:4:fn c() {}",
            ),
        ];
        for (method, format, input) in cases {
            assert!(
                method.info().idempotent,
                "{} must declare idempotent",
                method.name()
            );
            let ctx = MethodCtx { source: "test" };
            let once = run_chain(&[method], &testutil::block(input, format), &ctx);
            let twice = run_chain(&[method], &testutil::block(&once, format), &ctx);
            assert_eq!(once, twice, "{} is not idempotent", method.name());
        }
    }
}

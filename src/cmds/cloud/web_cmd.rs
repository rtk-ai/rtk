//! Fetches a URL and converts HTML responses into compact, LLM-friendly Markdown.
//!
//! Implements the "RTK Web Hook" proposed in #1426 as a narrow, explicit v1
//! (`rtk web <url>`, Option A from the issue) rather than a transparent
//! `curl` rewrite: HTML pages are full of `<script>`/`<style>`/navigation
//! noise that costs tokens without adding meaning, but blindly rewriting
//! every `curl` call risks corrupting JSON/XML/binary responses. Keeping it
//! an opt-in command sidesteps that risk entirely for v1; automatic rewriting
//! can be revisited later per the issue's own suggested rollout.
//!
//! Mirrors `curl_cmd`'s safety rails: non-HTML content (JSON, XML, plain
//! text, binary) and any content that fails to look like HTML is passed
//! through unchanged.

use crate::core::runner;
use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command};
use anyhow::{Context, Result};
use encoding_rs::{Encoding, UTF_8, WINDOWS_1252};
use scraper::{ElementRef, Html, Node, Selector};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let Some(url) = args.first() else {
        eprintln!("Usage: rtk web <url>");
        return Ok(1);
    };

    let header_file = tempfile::NamedTempFile::new().context("Failed to create temp file")?;

    let mut cmd = resolved_command("curl");
    cmd.arg("-s").arg("-L").arg("-D").arg(header_file.path());
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: curl -s -L -D <tmp> {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run curl")?;
    // Uses the shared signal-aware helper rather than `.code().unwrap_or(1)` —
    // the latter silently reports 1 for a signal-killed curl (e.g. timeout
    // SIGTERM) instead of the conventional 128+signal, the same class of bug
    // fixed for `rtk run` in #2681. `curl_cmd` still has the old pattern; not
    // touching it here since that's a pre-existing, separately-scoped issue.
    let exit_code = exit_code_from_output(&output, "web");

    // Skip filtering on failure: curl can return an HTML error body that
    // would be misleading to summarize, and we want the real exit code
    // surfaced (mirrors curl_cmd's failure handling).
    if !output.status.success() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr_str.trim().is_empty() {
            stdout_str.trim().to_string()
        } else {
            stderr_str.trim().to_string()
        };
        eprintln!("FAILED: web {}", msg);
        return Ok(exit_code);
    }

    let headers = std::fs::read_to_string(header_file.path()).unwrap_or_default();

    // HTML detection must happen BEFORE the binary check: a legacy-charset
    // page (windows-1250/1252, shift_jis, ...) contains non-UTF-8 bytes and
    // would otherwise be misclassified as binary and dumped raw — silently
    // producing zero savings on exactly the pages this command exists for.
    // The sniff prefix is lossy-decoded, which is safe because every signal
    // it looks for (doctype, `<html`, `charset=`) is pure ASCII and legacy
    // encodings are ASCII-compatible.
    let sniff_prefix = String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(1024)]);

    if !looks_like_html(&headers, &sniff_prefix) {
        // Non-goal for v1 (#1426): only HTML is transformed. JSON, XML, plain
        // text and binary all pass through byte-for-byte. Binary must bypass
        // the lossy conversion that would corrupt it with U+FFFD.
        if is_binary(&output.stdout) {
            std::io::stdout()
                .write_all(&output.stdout)
                .context("Failed to write response to stdout")?;
            timer.track_passthrough(&format!("curl {}", url), &format!("rtk web {}", url));
            return Ok(exit_code);
        }
        let raw = String::from_utf8_lossy(&output.stdout).into_owned();
        let shown = runner::emit_guarded(&raw, None, &raw);
        timer.track(
            &format!("curl {}", url),
            &format!("rtk web {}", url),
            &raw,
            &shown,
        );
        return Ok(exit_code);
    }

    let encoding = detect_encoding(&headers, &output.stdout);
    // decode() never fails — undecodable byte sequences become U+FFFD — and
    // handles a BOM (which overrides the detected label, per the standard).
    let (decoded, _, _) = encoding.decode(&output.stdout);
    let raw = decoded.into_owned();
    let content = html_to_markdown(&raw);

    // `emit_guarded` applies the same never-worse safety net `curl_cmd` uses
    // (see `core::guard::never_worse`): if the Markdown conversion ever came
    // out larger than the raw response — pathological/adversarial HTML — the
    // raw body is shown instead, so `rtk web` can never regress to costing
    // more tokens than plain `curl` would have.
    let shown = runner::emit_guarded(&content, None, &raw);
    timer.track(
        &format!("curl {}", url),
        &format!("rtk web {}", url),
        &raw,
        &shown,
    );

    Ok(exit_code)
}

/// Returns `true` if `bytes` is not valid UTF-8 (see `curl_cmd::is_binary` —
/// duplicated rather than shared because the two modules are expected to
/// diverge further, e.g. `web` will eventually sniff more content types).
fn is_binary(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_err()
}

/// Decides whether `body` should go through the HTML->Markdown pipeline.
///
/// Prefers the `Content-Type` header from the *final* response in the
/// redirect chain (curl's `-D` with `-L` appends one header block per hop).
/// Falls back to sniffing the body for a small set of unambiguous HTML
/// signals when no usable header is present — e.g. a proxy stripped it, or
/// the server just didn't send one.
fn looks_like_html(headers: &str, body: &str) -> bool {
    if let Some(content_type) = last_header_value(headers, "content-type") {
        let content_type = content_type.to_ascii_lowercase();
        if content_type.contains("html") {
            return true;
        }
        // Explicit non-HTML content type: trust it over body sniffing so a
        // JSON/XML API response is never misdetected as HTML because it
        // happens to contain a "<" character somewhere in a string value.
        if !content_type.is_empty() {
            return false;
        }
    }

    let trimmed = body.trim_start();
    let head: String = trimmed.chars().take(512).collect::<String>().to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// Returns the value of the last occurrence of `name` across all header
/// blocks in `headers` (one block per redirect hop), case-insensitive.
fn last_header_value(headers: &str, name: &str) -> Option<String> {
    let normalized = headers.replace("\r\n", "\n");
    let last_block = normalized
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .last()?;

    let prefix = format!("{}:", name.to_ascii_lowercase());
    last_block.lines().find_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower
            .starts_with(&prefix)
            .then(|| line[prefix.len()..].trim().to_string())
    })
}

/// Resolves the encoding to decode an HTML body with, in standard priority
/// order: `charset=` in the Content-Type header, then a `<meta charset>` /
/// `http-equiv` declaration in the first 1024 bytes (a simplified version of
/// the WHATWG prescan), then UTF-8 if the body validates as it, and finally
/// windows-1252 — the Encoding Standard's fallback for undeclared legacy
/// content, matching what browsers do.
fn detect_encoding(headers: &str, body: &[u8]) -> &'static Encoding {
    let header_charset = last_header_value(headers, "content-type")
        .and_then(|ct| extract_charset(&ct))
        .and_then(|label| Encoding::for_label(label.as_bytes()));
    if let Some(enc) = header_charset {
        return enc;
    }

    // Legacy encodings are ASCII-compatible, so a lossy prefix is safe for
    // locating the (pure-ASCII) meta declaration.
    let prefix = String::from_utf8_lossy(&body[..body.len().min(1024)]);
    if let Some(enc) = extract_charset(&prefix).and_then(|label| Encoding::for_label(label.as_bytes()))
    {
        return enc;
    }

    if std::str::from_utf8(body).is_ok() {
        UTF_8
    } else {
        WINDOWS_1252
    }
}

/// Pulls the value of the first `charset=` occurrence out of `text` (a
/// Content-Type header value or an HTML prefix), tolerating optional quotes
/// and whitespace. Returns `None` when no declaration is present.
fn extract_charset(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("charset")? + "charset".len();
    let rest = lower[start..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == '"' || c == '\'' || c == ';' || c == '>' || c.is_whitespace())
        .unwrap_or(rest.len());
    let label = rest[..end].trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// Tags whose entire subtree carries no reader-facing content.
const SKIP_SUBTREE: &[&str] = &[
    "script",
    "style",
    "noscript",
    "template",
    "head",
    "iframe",
    "svg",
    "form",
    "button",
    "nav",
    "footer",
    "aside",
];

/// ARIA landmark roles that mark a subtree as page chrome rather than
/// content — the attribute-level equivalent of the `nav`/`footer`/`aside`
/// entries in [`SKIP_SUBTREE`], for sites that use `<div role="navigation">`
/// instead of semantic tags.
const SKIP_ROLES: &[&str] = &[
    "navigation",
    "banner",
    "complementary",
    "contentinfo",
    "search",
];

/// id/class tokens that mark an element as page chrome (what readability
/// implementations score down). Matched against whole tokens after splitting
/// on whitespace, `-` and `_`, so `mw-footer` and `vector-menu` are skipped
/// while `navy-blue` (token `navy`) survives.
const SKIP_CLASS_TOKENS: &[&str] = &[
    "nav",
    "navbar",
    "navigation",
    "menu",
    "dropdown",
    "sidebar",
    "breadcrumb",
    "breadcrumbs",
    "banner",
    "cookie",
    "cookies",
    "footer",
    "toc",
];

/// Returns `true` when an element's attributes identify it as page chrome
/// (navigation, banners, cookie bars, ...) that should be pruned wholesale.
fn is_chrome_element(el: &scraper::node::Element) -> bool {
    if let Some(role) = el.attr("role") {
        let role = role.to_ascii_lowercase();
        if SKIP_ROLES.contains(&role.as_str()) {
            return true;
        }
    }
    ["id", "class"].iter().any(|attr| {
        el.attr(attr).is_some_and(|value| {
            let lower = value.to_ascii_lowercase();
            lower
                .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
                .any(|token| SKIP_CLASS_TOKENS.contains(&token))
        })
    })
}

/// Converts an HTML document into compact Markdown: strips non-content
/// subtrees, extracts the main content container, preserves headings /
/// lists / code blocks / emphasis, and de-duplicates links into numbered
/// references (`[1]`, `[2]`, ...) instead of repeating long URLs inline.
///
/// html5ever-backed parsing (via `scraper`) never fails hard on malformed
/// markup — it degrades gracefully instead of erroring — so this always
/// returns a String; there is no fallible path to a raw-passthrough here.
fn html_to_markdown(raw: &str) -> String {
    let document = Html::parse_document(raw);
    let root = select_main_content(&document);

    let mut ctx = MarkdownContext::default();
    walk(*root, &mut ctx);
    ctx.finish()
}

fn select_main_content(document: &Html) -> ElementRef<'_> {
    for selector in ["main", "article", "[role=\"main\"]"] {
        if let Some(el) = Selector::parse(selector)
            .ok()
            .and_then(|s| document.select(&s).next())
        {
            return el;
        }
    }
    Selector::parse("body")
        .ok()
        .and_then(|s| document.select(&s).next())
        .unwrap_or_else(|| document.root_element())
}

#[derive(Default)]
struct MarkdownContext {
    out: String,
    links: Vec<String>,
    link_index: HashMap<String, usize>,
    in_pre: usize,
    /// One entry per open list: `Some(count)` for `<ol>` (next item number),
    /// `None` for `<ul>`. Depth drives nested-list indentation.
    list_stack: Vec<Option<usize>>,
}

impl MarkdownContext {
    /// Ensures the next write starts on a fresh paragraph (single blank
    /// line between blocks, never more, regardless of how much trailing
    /// whitespace the source HTML had).
    fn open_block(&mut self) {
        let trimmed = self.out.trim_end_matches(' ');
        let extra_newlines = trimmed.len() - trimmed.trim_end_matches('\n').len();
        self.out.truncate(trimmed.trim_end_matches('\n').len());
        if !self.out.is_empty() {
            self.out.push('\n');
            self.out.push('\n');
        }
        let _ = extra_newlines; // trailing newlines already normalized above
    }

    fn push_inline(&mut self, text: &str) {
        if self.in_pre > 0 {
            self.out.push_str(text);
            return;
        }
        // HTML collapses runs of whitespace (including newlines) to a
        // single space outside <pre>; replicate that here.
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            return;
        }
        let needs_space = text.starts_with(char::is_whitespace)
            && !self.out.is_empty()
            && !self.out.ends_with(['\n', ' ']);
        if needs_space {
            self.out.push(' ');
        }
        self.out.push_str(&collapsed);
    }

    fn link_ref(&mut self, href: &str) -> usize {
        if let Some(&idx) = self.link_index.get(href) {
            return idx;
        }
        self.links.push(href.to_string());
        let idx = self.links.len();
        self.link_index.insert(href.to_string(), idx);
        idx
    }

    fn finish(self) -> String {
        let body = self.out.trim().to_string();
        if self.links.is_empty() {
            return body;
        }
        let mut out = body;
        out.push_str("\n\n");
        for (i, href) in self.links.iter().enumerate() {
            let _ = writeln!(out, "[{}] {}", i + 1, href);
        }
        out
    }
}

fn walk(node: ego_tree::NodeRef<'_, Node>, ctx: &mut MarkdownContext) {
    match node.value() {
        Node::Text(text) => ctx.push_inline(text),
        Node::Element(el) => {
            let tag = el.name();
            if SKIP_SUBTREE.contains(&tag) || is_chrome_element(el) {
                return;
            }

            match tag {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    ctx.open_block();
                    let level = tag.as_bytes()[1] - b'0';
                    ctx.out.push_str(&"#".repeat(level as usize));
                    ctx.out.push(' ');
                    walk_children(node, ctx);
                    return;
                }
                "p" | "div" | "section" | "article" | "main" | "blockquote" => {
                    ctx.open_block();
                    walk_children(node, ctx);
                    return;
                }
                "li" => {
                    ctx.open_block();
                    let depth = ctx.list_stack.len().saturating_sub(1);
                    ctx.out.push_str(&"  ".repeat(depth));
                    match ctx.list_stack.last_mut() {
                        Some(Some(n)) => {
                            *n += 1;
                            let marker = format!("{}. ", n);
                            ctx.out.push_str(&marker);
                        }
                        _ => ctx.out.push_str("- "),
                    }
                    walk_children(node, ctx);
                    return;
                }
                "ul" => {
                    ctx.open_block();
                    ctx.list_stack.push(None);
                    walk_children(node, ctx);
                    ctx.list_stack.pop();
                    return;
                }
                "ol" => {
                    ctx.open_block();
                    ctx.list_stack.push(Some(0));
                    walk_children(node, ctx);
                    ctx.list_stack.pop();
                    return;
                }
                "table" | "tr" => {
                    ctx.open_block();
                    walk_children(node, ctx);
                    return;
                }
                "td" | "th" => {
                    // Pipe-separated cells on one row per `tr` — plain rows,
                    // no alignment header; enough to stop adjacent cell text
                    // fusing into one word.
                    if !ctx.out.is_empty() && !ctx.out.ends_with('\n') {
                        ctx.out.push_str(" | ");
                    }
                    walk_children(node, ctx);
                    return;
                }
                "pre" => {
                    ctx.open_block();
                    ctx.out.push_str("```\n");
                    ctx.in_pre += 1;
                    walk_children(node, ctx);
                    ctx.in_pre -= 1;
                    ctx.out.push_str("\n```");
                    return;
                }
                "code" if ctx.in_pre == 0 => {
                    ctx.out.push('`');
                    walk_children(node, ctx);
                    ctx.out.push('`');
                    return;
                }
                "strong" | "b" => {
                    ctx.out.push_str("**");
                    walk_children(node, ctx);
                    ctx.out.push_str("**");
                    return;
                }
                "em" | "i" => {
                    ctx.out.push('*');
                    walk_children(node, ctx);
                    ctx.out.push('*');
                    return;
                }
                "br" => {
                    ctx.out.push('\n');
                    return;
                }
                "hr" => {
                    ctx.open_block();
                    ctx.out.push_str("---");
                    return;
                }
                "a" => {
                    if let Some(href) = el.attr("href") {
                        let start = ctx.out.len();
                        walk_children(node, ctx);
                        let text = ctx.out[start..].to_string();
                        ctx.out.truncate(start);
                        let label = if text.trim().is_empty() {
                            href.to_string()
                        } else {
                            text
                        };
                        let idx = ctx.link_ref(href);
                        let _ = write!(ctx.out, "[{}][{}]", label, idx);
                        return;
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    walk_children(node, ctx);
}

fn walk_children(node: ego_tree::NodeRef<'_, Node>, ctx: &mut MarkdownContext) {
    for child in node.children() {
        walk(child, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_html_from_content_type_header() {
        let headers = "HTTP/1.1 200 OK\nContent-Type: text/html; charset=utf-8\n\n";
        assert!(looks_like_html(headers, "irrelevant"));
    }

    #[test]
    fn test_looks_like_html_rejects_json_content_type() {
        let headers = "HTTP/1.1 200 OK\nContent-Type: application/json\n\n";
        assert!(!looks_like_html(headers, "<html so this would false-positive on sniff>"));
    }

    #[test]
    fn test_looks_like_html_uses_last_redirect_hop() {
        let headers = "HTTP/1.1 301 Moved\nContent-Type: text/plain\nLocation: /next\n\nHTTP/1.1 200 OK\nContent-Type: text/html\n\n";
        assert!(looks_like_html(headers, "irrelevant"));
    }

    #[test]
    fn test_looks_like_html_sniffs_body_without_header() {
        assert!(looks_like_html("", "<!DOCTYPE html><html><body>hi</body></html>"));
        assert!(!looks_like_html("", "{\"key\": \"value\"}"));
    }

    #[test]
    fn test_html_to_markdown_strips_script_and_style() {
        let html = r#"<html><head><style>.x{color:red}</style></head>
            <body><script>alert(1)</script><main><p>Hello world</p></main></body></html>"#;
        let md = html_to_markdown(html);
        assert_eq!(md, "Hello world");
    }

    #[test]
    fn test_html_to_markdown_headings_and_lists() {
        let html = r#"<html><body><main>
            <h1>Title</h1>
            <ul><li>One</li><li>Two</li></ul>
        </main></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("- One"));
        assert!(md.contains("- Two"));
    }

    #[test]
    fn test_html_to_markdown_dedupes_links_into_references() {
        let html = r#"<html><body><article>
            <p><a href="https://example.com/page">first</a></p>
            <p><a href="https://example.com/page">second</a></p>
        </article></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("[first][1]"));
        assert!(md.contains("[second][1]"));
        assert_eq!(md.matches("https://example.com/page").count(), 1);
    }

    #[test]
    fn test_html_to_markdown_prefers_main_over_nav() {
        let html = r#"<html><body>
            <nav><a href="/a">Nav link</a></nav>
            <main><p>Real content</p></main>
            <footer>Copyright</footer>
        </body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("Real content"));
        assert!(!md.contains("Nav link"));
        assert!(!md.contains("Copyright"));
    }

    #[test]
    fn test_html_to_markdown_preserves_code_blocks() {
        let html = r#"<html><body><main><pre><code>fn main() {}</code></pre></main></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("```"));
        assert!(md.contains("fn main() {}"));
    }

    #[test]
    fn test_html_to_markdown_never_panics_on_malformed_html() {
        let inputs = [
            "<html><body><p>unclosed",
            "<div><span><p></div></span>",
            "not html at all, just text",
            "",
            "<html>&amp;&lt;broken&gt;entities&amp;</html>",
        ];
        for input in inputs {
            let _ = html_to_markdown(input);
        }
    }

    #[test]
    fn test_is_binary_matches_curl_cmd_semantics() {
        assert!(is_binary(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(!is_binary(b"<html></html>"));
    }

    #[test]
    fn test_html_to_markdown_ordered_lists_numbered() {
        let html = "<html><body><main><ol><li>Alpha</li><li>Beta</li></ol><ul><li>Dash</li></ul></main></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("1. Alpha"));
        assert!(md.contains("2. Beta"));
        assert!(md.contains("- Dash"));
    }

    #[test]
    fn test_html_to_markdown_nested_lists_indented() {
        let html = "<html><body><main><ul><li>Outer<ul><li>Inner</li></ul></li></ul></main></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("- Outer"));
        assert!(md.contains("  - Inner"));
    }

    #[test]
    fn test_html_to_markdown_table_cells_separated() {
        let html = "<html><body><main><table><tr><th>Name</th><th>Age</th></tr><tr><td>Ada</td><td>36</td></tr></table></main></body></html>";
        let md = html_to_markdown(html);
        assert!(md.contains("Name | Age"), "th cells must not fuse: {md}");
        assert!(md.contains("Ada | 36"), "td cells must not fuse: {md}");
    }

    #[test]
    fn test_html_to_markdown_skips_chrome_by_class_id_and_role() {
        let html = r#"<html><body><main>
            <div class="vector-menu mw-portlet">Language switcher junk</div>
            <div id="site-footer-inner">Footer junk</div>
            <div role="navigation">More nav junk</div>
            <div class="navy-blue">Navy blue is fine</div>
            <p>Article text</p>
        </main></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("Article text"));
        assert!(
            md.contains("Navy blue is fine"),
            "token matching must not fire on substrings like navy"
        );
        assert!(!md.contains("junk"));
    }

    #[test]
    fn test_extract_charset_variants() {
        assert_eq!(
            extract_charset("text/html; charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            extract_charset("text/html; charset=\"windows-1250\"").as_deref(),
            Some("windows-1250")
        );
        assert_eq!(
            extract_charset("<meta charset='iso-8859-2'>").as_deref(),
            Some("iso-8859-2")
        );
        assert_eq!(extract_charset("text/html"), None);
    }

    #[test]
    fn test_detect_encoding_header_charset_wins_over_meta() {
        let headers = "HTTP/1.1 200 OK\nContent-Type: text/html; charset=windows-1250\n\n";
        let body = b"<meta charset=\"utf-8\"><p>x</p>";
        assert_eq!(detect_encoding(headers, body).name(), "windows-1250");
    }

    #[test]
    fn test_detect_encoding_meta_fallback_when_header_has_no_charset() {
        let headers = "HTTP/1.1 200 OK\nContent-Type: text/html\n\n";
        let body =
            b"<!DOCTYPE html><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1250\">";
        assert_eq!(detect_encoding(headers, body).name(), "windows-1250");
    }

    #[test]
    fn test_detect_encoding_utf8_then_1252_fallback() {
        let headers = "HTTP/1.1 200 OK\nContent-Type: text/html\n\n";
        assert_eq!(
            detect_encoding(headers, b"<html>plain ascii</html>").name(),
            "UTF-8"
        );
        // 0xE9 = 'e-acute' in windows-1252 — an invalid byte sequence as UTF-8.
        assert_eq!(
            detect_encoding(headers, b"<html>caf\xE9</html>").name(),
            "windows-1252"
        );
    }

    /// The regression this phase exists for: a windows-1250 page contains
    /// non-UTF-8 bytes, so the old order (binary check before HTML detection)
    /// dumped it raw with zero savings. Decoding must produce correct Czech
    /// diacritics, not U+FFFD.
    #[test]
    fn test_windows_1250_page_converts_instead_of_passthrough() {
        let html = "<html><body><main><p>Příliš žluťoučký kůň úpěl ďábelské ódy</p></main></body></html>";
        let (bytes, _, _) = encoding_rs::WINDOWS_1250.encode(html);
        assert!(
            is_binary(&bytes),
            "fixture must be non-UTF-8 or it proves nothing"
        );
        let headers = "HTTP/1.1 200 OK\nContent-Type: text/html; charset=windows-1250\n\n";
        let sniff = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]);
        assert!(looks_like_html(headers, &sniff));
        let (decoded, _, _) = detect_encoding(headers, &bytes).decode(&bytes);
        let md = html_to_markdown(&decoded);
        assert_eq!(md, "Příliš žluťoučký kůň úpěl ďábelské ódy");
    }

    /// A page-shaped fixture — nav/header/footer chrome, inline scripts and
    /// styles, and repeated long links around a real content block — is the
    /// exact scenario #1426 exists to fix. Verified against live pages during
    /// development (example.com ~69%, a Wikipedia article ~83%); this fixture
    /// pins a deterministic floor so the savings claim doesn't regress silently.
    #[test]
    fn test_html_to_markdown_meets_token_savings_floor() {
        let raw = include_str!("web_cmd_fixture.html");
        let markdown = html_to_markdown(raw);

        let count_tokens = |s: &str| s.split_whitespace().count();
        let input_tokens = count_tokens(raw);
        let output_tokens = count_tokens(&markdown);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "web: expected >=60% token savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
        assert!(markdown.contains("Article Heading"));
        assert!(markdown.contains("first paragraph"));
        assert!(!markdown.contains("Home"), "nav chrome should be stripped");
        assert!(
            !markdown.contains("Copyright"),
            "footer chrome should be stripped"
        );
    }
}

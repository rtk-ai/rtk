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

use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
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
    let exit_code = output.status.code().unwrap_or(1);

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

    // Binary detection first: from_utf8_lossy would corrupt any non-UTF-8
    // payload (images, archives, ...) by replacing invalid bytes with U+FFFD.
    if is_binary(&output.stdout) {
        std::io::stdout()
            .write_all(&output.stdout)
            .context("Failed to write response to stdout")?;
        timer.track_passthrough(&format!("curl {}", url), &format!("rtk web {}", url));
        return Ok(exit_code);
    }

    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let headers = std::fs::read_to_string(header_file.path()).unwrap_or_default();

    // Non-goal for v1 (#1426): only HTML is transformed. JSON, XML, plain
    // text and anything else passes through byte-for-byte.
    if !looks_like_html(&headers, &raw) {
        print!("{raw}");
        std::io::stdout().flush().ok();
        timer.track_passthrough(&format!("curl {}", url), &format!("rtk web {}", url));
        return Ok(exit_code);
    }

    let markdown = html_to_markdown(&raw);
    print!("{markdown}");
    std::io::stdout().flush().ok();
    timer.track(
        &format!("curl {}", url),
        &format!("rtk web {}", url),
        &raw,
        &markdown,
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
            if SKIP_SUBTREE.contains(&tag) {
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
                    ctx.out.push_str("- ");
                    walk_children(node, ctx);
                    return;
                }
                "ul" | "ol" => {
                    ctx.open_block();
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

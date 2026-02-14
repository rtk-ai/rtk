use crate::tracking;
use anyhow::{bail, Result};
use ignore::WalkBuilder;
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_SNIPPETS_PER_FILE: usize = 2;
const MAX_SNIPPET_LINE_LEN: usize = 140;
const MIN_FILE_SCORE: f64 = 2.4;

const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "code", "file", "find", "for", "from", "how",
    "in", "is", "it", "of", "on", "or", "search", "show", "that", "the", "this", "to", "use",
    "using", "what", "when", "where", "with", "why",
];

lazy_static! {
    static ref SYMBOL_DEF_RE: Regex = Regex::new(
        r"^\s*(?:pub\s+)?(?:async\s+)?(?:fn|def|class|struct|enum|trait|interface|impl|type)\s+[A-Za-z_][A-Za-z0-9_]*"
    )
    .expect("valid symbol regex");
}

#[derive(Debug, Clone)]
struct QueryModel {
    phrase: String,
    terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct LineCandidate {
    line_idx: usize,
    score: f64,
    matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct Snippet {
    lines: Vec<(usize, String)>,
    matched_terms: Vec<String>,
}

#[derive(Debug, Clone)]
struct SearchHit {
    path: String,
    score: f64,
    matched_lines: usize,
    snippets: Vec<Snippet>,
}

#[derive(Debug, Default)]
struct SearchOutcome {
    scanned_files: usize,
    skipped_large: usize,
    skipped_binary: usize,
    hits: Vec<SearchHit>,
    raw_output: String,
}

pub fn run(
    query: &str,
    path: &str,
    max_results: usize,
    context_lines: usize,
    file_type: Option<&str>,
    max_file_kb: usize,
    json_output: bool,
    compact: bool,
    verbose: u8,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let query = query.trim();
    if query.is_empty() {
        bail!("query cannot be empty");
    }

    let root = Path::new(path);
    if !root.exists() {
        bail!("path does not exist: {}", path);
    }

    let query_model = build_query_model(query);
    if verbose > 0 {
        eprintln!(
            "rgai: '{}' in {} (terms: {})",
            query,
            path,
            query_model.terms.join(", ")
        );
    }

    let max_file_bytes = max_file_kb.saturating_mul(1024).max(1024);
    let effective_context = if compact { 0 } else { context_lines };
    let snippets_per_file = if compact { 1 } else { MAX_SNIPPETS_PER_FILE };
    let outcome = search_project(
        &query_model,
        root,
        effective_context,
        snippets_per_file,
        file_type,
        max_file_bytes,
        verbose,
    )?;

    let mut rendered = String::new();
    if outcome.hits.is_empty() {
        if json_output {
            rendered = serde_json::to_string_pretty(&json!({
                "query": query,
                "path": path,
                "total_hits": 0,
                "scanned_files": outcome.scanned_files,
                "skipped_large": outcome.skipped_large,
                "skipped_binary": outcome.skipped_binary,
                "hits": []
            }))?;
            rendered.push('\n');
        } else {
            rendered.push_str(&format!("🧠 0 for '{}'\n", query));
        }
        print!("{}", rendered);
        timer.track(
            &format!("grepai search '{}' {}", query, path),
            "rtk rgai",
            &outcome.raw_output,
            &rendered,
        );
        return Ok(());
    }

    if json_output {
        let hits_json: Vec<_> = outcome
            .hits
            .iter()
            .take(max_results)
            .map(|hit| {
                let snippets: Vec<_> = hit
                    .snippets
                    .iter()
                    .map(|snippet| {
                        let lines: Vec<_> = snippet
                            .lines
                            .iter()
                            .map(|(line_no, text)| json!({ "line": line_no, "text": text }))
                            .collect();
                        json!({
                            "lines": lines,
                            "matched_terms": snippet.matched_terms,
                        })
                    })
                    .collect();
                json!({
                    "path": hit.path,
                    "score": hit.score,
                    "matched_lines": hit.matched_lines,
                    "snippets": snippets,
                })
            })
            .collect();

        rendered = serde_json::to_string_pretty(&json!({
            "query": query,
            "path": path,
            "total_hits": outcome.hits.len(),
            "shown_hits": max_results.min(outcome.hits.len()),
            "scanned_files": outcome.scanned_files,
            "skipped_large": outcome.skipped_large,
            "skipped_binary": outcome.skipped_binary,
            "hits": hits_json
        }))?;
        rendered.push('\n');
        print!("{}", rendered);
        timer.track(
            &format!("grepai search '{}' {}", query, path),
            "rtk rgai",
            &outcome.raw_output,
            &rendered,
        );
        return Ok(());
    }

    rendered.push_str(&format!(
        "🧠 {}F for '{}' (scan {}F)\n",
        outcome.hits.len(),
        query,
        outcome.scanned_files
    ));
    rendered.push('\n');

    for hit in outcome.hits.iter().take(max_results) {
        rendered.push_str(&format!(
            "📄 {} [{:.1}]\n",
            compact_path(&hit.path),
            hit.score
        ));

        for snippet in &hit.snippets {
            for (line_no, line) in &snippet.lines {
                rendered.push_str(&format!("  {:>4}: {}\n", line_no, line));
            }

            if !compact && !snippet.matched_terms.is_empty() {
                rendered.push_str(&format!("       ~ {}\n", snippet.matched_terms.join(", ")));
            }
            rendered.push('\n');
        }

        let shown_lines = hit.snippets.len();
        if hit.matched_lines > shown_lines {
            rendered.push_str(&format!(
                "  +{} more lines\n\n",
                hit.matched_lines - shown_lines
            ));
        }
    }

    if outcome.hits.len() > max_results {
        rendered.push_str(&format!("... +{}F\n", outcome.hits.len() - max_results));
    }

    if verbose > 0 {
        rendered.push_str(&format!(
            "\nscan stats: skipped {} large, {} binary\n",
            outcome.skipped_large, outcome.skipped_binary
        ));
    }

    print!("{}", rendered);
    timer.track(
        &format!("grepai search '{}' {}", query, path),
        "rtk rgai",
        &outcome.raw_output,
        &rendered,
    );

    Ok(())
}

fn search_project(
    query: &QueryModel,
    root: &Path,
    context_lines: usize,
    snippets_per_file: usize,
    file_type: Option<&str>,
    max_file_bytes: usize,
    _verbose: u8,
) -> Result<SearchOutcome> {
    let mut outcome = SearchOutcome::default();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry
            .file_type()
            .as_ref()
            .map(|ft| ft.is_file())
            .unwrap_or(false)
        {
            continue;
        }

        let full_path = entry.path();
        if !is_supported_text_file(full_path) {
            continue;
        }

        if let Some(ft) = file_type {
            if !matches_file_type(full_path, ft) {
                continue;
            }
        }

        let metadata = match fs::metadata(full_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        outcome.scanned_files += 1;

        if metadata.len() > max_file_bytes as u64 {
            outcome.skipped_large += 1;
            continue;
        }

        let bytes = match fs::read(full_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        if looks_binary(&bytes) {
            outcome.skipped_binary += 1;
            continue;
        }

        let content = String::from_utf8_lossy(&bytes).to_string();
        let display_path = compact_display_path(full_path, root);
        if let Some(hit) = analyze_file(
            &display_path,
            &content,
            query,
            context_lines,
            snippets_per_file,
        ) {
            outcome.hits.push(hit);
        }
    }

    outcome.hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });

    outcome.raw_output = build_raw_output(&outcome.hits);
    Ok(outcome)
}

fn analyze_file(
    path: &str,
    content: &str,
    query: &QueryModel,
    context_lines: usize,
    snippets_per_file: usize,
) -> Option<SearchHit> {
    let mut candidates = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        if let Some(candidate) = score_line(idx, line, query) {
            candidates.push(candidate);
        }
    }

    let path_score = score_path(path, query);
    if candidates.is_empty() && path_score < MIN_FILE_SCORE {
        return None;
    }

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.line_idx.cmp(&b.line_idx))
    });

    let mut selected = Vec::new();
    let overlap_window = (context_lines * 2 + 1) as isize;
    for cand in candidates.iter().cloned() {
        let overlaps = selected.iter().any(|existing: &LineCandidate| {
            let delta = existing.line_idx as isize - cand.line_idx as isize;
            delta.abs() <= overlap_window
        });
        if overlaps {
            continue;
        }
        selected.push(cand);
        if selected.len() >= snippets_per_file {
            break;
        }
    }

    if selected.is_empty() {
        return None;
    }

    let lines: Vec<&str> = content.lines().collect();
    let mut snippets = Vec::new();
    for cand in &selected {
        snippets.push(build_snippet(&lines, cand, context_lines));
    }

    let mut file_score = path_score + (candidates.len() as f64).ln_1p();
    for (idx, cand) in selected.iter().enumerate() {
        let weight = match idx {
            0 => 1.0,
            1 => 0.45,
            _ => 0.25,
        };
        file_score += cand.score * weight;
    }

    if file_score < MIN_FILE_SCORE {
        return None;
    }

    Some(SearchHit {
        path: path.to_string(),
        score: file_score,
        matched_lines: candidates.len(),
        snippets,
    })
}

fn build_snippet(lines: &[&str], candidate: &LineCandidate, context_lines: usize) -> Snippet {
    if lines.is_empty() {
        return Snippet {
            lines: vec![(candidate.line_idx + 1, String::new())],
            matched_terms: candidate.matched_terms.clone(),
        };
    }

    let start = candidate.line_idx.saturating_sub(context_lines);
    let end = (candidate.line_idx + context_lines + 1).min(lines.len());
    let mut rendered_lines = Vec::new();

    for (idx, line) in lines.iter().enumerate().take(end).skip(start) {
        let cleaned = line.trim();
        if cleaned.is_empty() {
            continue;
        }
        rendered_lines.push((idx + 1, truncate_chars(cleaned, MAX_SNIPPET_LINE_LEN)));
    }

    if rendered_lines.is_empty() {
        rendered_lines.push((candidate.line_idx + 1, String::new()));
    }

    Snippet {
        lines: rendered_lines,
        matched_terms: candidate.matched_terms.clone(),
    }
}

fn build_raw_output(hits: &[SearchHit]) -> String {
    let mut raw = String::new();
    for hit in hits.iter().take(60) {
        for snippet in &hit.snippets {
            for (line_no, line) in &snippet.lines {
                raw.push_str(&format!("{}:{}:{}\n", hit.path, line_no, line));
            }
        }
    }
    raw
}

fn score_line(line_idx: usize, line: &str, query: &QueryModel) -> Option<LineCandidate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_lowercase();
    let mut score = 0.0;
    let mut matched_terms = Vec::new();

    if query.phrase.len() >= 3 && lower.contains(&query.phrase) {
        score += 6.0;
    }

    for term in &query.terms {
        if lower.contains(term) {
            score += if term.len() >= 5 { 1.7 } else { 1.4 };
            matched_terms.push(term.clone());
        }
    }

    let unique_matches = dedup_terms(matched_terms);
    if unique_matches.is_empty() {
        return None;
    }

    if unique_matches.len() > 1 {
        score += 1.2;
    }

    if is_symbol_definition(trimmed) {
        score += 2.5;
    }

    if is_comment_line(trimmed) {
        score *= 0.7;
    }

    if trimmed.chars().count() > 220 {
        score *= 0.9;
    }

    if score < 1.2 {
        return None;
    }

    Some(LineCandidate {
        line_idx,
        score,
        matched_terms: unique_matches,
    })
}

fn score_path(path: &str, query: &QueryModel) -> f64 {
    let lower = path.to_lowercase();
    let mut score = 0.0;

    if query.phrase.len() >= 3 && lower.contains(&query.phrase) {
        score += 3.5;
    }

    for term in &query.terms {
        if lower.contains(term) {
            score += 1.2;
        }
    }

    score
}

fn build_query_model(query: &str) -> QueryModel {
    let phrase = query.trim().to_lowercase();
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for token in split_terms(&phrase) {
        if token.len() < 2 || STOP_WORDS.contains(&token.as_str()) {
            continue;
        }
        push_unique(&mut terms, &mut seen, &token);

        let stemmed = stem_token(&token);
        if stemmed != token && stemmed.len() >= 2 {
            push_unique(&mut terms, &mut seen, &stemmed);
        }
    }

    if terms.is_empty() && !phrase.is_empty() {
        terms.push(phrase.clone());
    }

    QueryModel { phrase, terms }
}

fn split_terms(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.extend(ch.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn stem_token(token: &str) -> String {
    if !token.is_ascii() {
        return token.to_string();
    }

    let suffixes = ["ingly", "edly", "ing", "ed", "es", "s"];
    for suffix in suffixes {
        if token.len() > suffix.len() + 2 && token.ends_with(suffix) {
            return token[..token.len() - suffix.len()].to_string();
        }
    }
    token.to_string()
}

fn push_unique(out: &mut Vec<String>, seen: &mut HashSet<String>, item: &str) {
    if seen.insert(item.to_string()) {
        out.push(item.to_string());
    }
}

fn dedup_terms(input: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for item in input {
        if seen.insert(item.clone()) {
            out.push(item);
        }
    }
    out
}

fn is_symbol_definition(line: &str) -> bool {
    SYMBOL_DEF_RE.is_match(line)
}

fn is_comment_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//")
        || trimmed.starts_with('#')
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
        || trimmed.starts_with("--")
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|b| *b == 0)
}

fn is_supported_text_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    !matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
            | "7z"
            | "mp3"
            | "mp4"
            | "mov"
            | "db"
            | "sqlite"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "lock"
            | "jar"
            | "class"
            | "wasm"
    )
}

fn matches_file_type(path: &Path, file_type: &str) -> bool {
    let wanted = file_type.trim_start_matches('.').to_ascii_lowercase();
    if wanted.is_empty() {
        return true;
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    match wanted.as_str() {
        "rust" | "rs" => ext == "rs",
        "python" | "py" => ext == "py",
        "javascript" | "js" => matches!(ext.as_str(), "js" | "jsx" | "mjs" | "cjs"),
        "typescript" | "ts" => matches!(ext.as_str(), "ts" | "tsx"),
        "go" => ext == "go",
        "java" => ext == "java",
        "c" => matches!(ext.as_str(), "c" | "h"),
        "cpp" | "c++" => matches!(ext.as_str(), "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx"),
        "markdown" | "md" => matches!(ext.as_str(), "md" | "mdx"),
        "json" => ext == "json",
        other => ext == other,
    }
}

fn compact_display_path(path: &Path, root: &Path) -> String {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => {
            if let Ok(cwd) = std::env::current_dir() {
                match path.strip_prefix(cwd) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => PathBuf::from(path),
                }
            } else {
                PathBuf::from(path)
            }
        }
    };
    rel.to_string_lossy().trim_start_matches("./").to_string()
}

fn compact_path(path: &str) -> String {
    if path.len() <= 58 {
        return path.to_string();
    }

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= 3 {
        return path.to_string();
    }

    format!(
        "{}/.../{}/{}",
        parts[0],
        parts[parts.len() - 2],
        parts[parts.len() - 1]
    )
}

fn truncate_chars(input: &str, max_len: usize) -> String {
    if input.chars().count() <= max_len {
        return input.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }
    let clipped: String = input.chars().take(max_len - 3).collect();
    format!("{clipped}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_query_model_removes_stop_words() {
        let model = build_query_model("how to find auth token refresh");
        assert!(model.terms.contains(&"auth".to_string()));
        assert!(model.terms.contains(&"token".to_string()));
        assert!(model.terms.contains(&"refresh".to_string()));
        assert!(!model.terms.contains(&"how".to_string()));
        assert!(!model.terms.contains(&"find".to_string()));
    }

    #[test]
    fn score_line_prefers_symbol_definitions() {
        let query = build_query_model("refresh token");
        let line = "pub fn refresh_token(session: &Session) -> Result<String> {";
        let cand = score_line(10, line, &query).expect("line should match");
        assert!(cand.score > 3.0);
        assert!(cand.matched_terms.contains(&"refresh".to_string()));
        assert!(cand.matched_terms.contains(&"token".to_string()));
    }

    #[test]
    fn search_project_finds_most_relevant_file() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/auth.rs"),
            r#"
pub struct Session {}

pub fn refresh_token(session: &Session) -> String {
    format!("new-token-{}", 1)
}
"#,
        )
        .unwrap();
        fs::write(
            root.join("src/logger.rs"),
            r#"
pub fn log_info(msg: &str) {
    println!("{}", msg);
}
"#,
        )
        .unwrap();

        let query = build_query_model("refresh token session");
        let outcome = search_project(&query, root, 0, 2, None, 256 * 1024, 0).unwrap();

        assert!(!outcome.hits.is_empty());
        assert_eq!(outcome.hits[0].path, "src/auth.rs");
    }

    #[test]
    fn matches_file_type_aliases() {
        let p = Path::new("src/app.tsx");
        assert!(matches_file_type(p, "ts"));
        assert!(matches_file_type(p, "typescript"));
        assert!(!matches_file_type(p, "rust"));
    }

    #[test]
    fn truncate_chars_handles_unicode() {
        let s = "Привет это длинная строка для теста";
        let truncated = truncate_chars(s, 10);
        assert!(truncated.chars().count() <= 10);
    }
}

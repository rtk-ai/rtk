//! `rtk retrieve` — pull a parked stash blob back by handle, optionally sliced
//! or grepped so the agent can re-interrogate large output without paying for
//! the whole thing again.

use anyhow::{anyhow, Result};

use crate::core::stash::StashStore;

/// Entry point for the `rtk retrieve` subcommand.
///
/// Filters compose in a fixed order over the parked lines: grep → lines → head
/// → tail. `meta` short-circuits to a metadata dump.
pub fn run(
    handle: &str,
    lines: Option<String>,
    grep: Option<String>,
    head: Option<usize>,
    tail: Option<usize>,
    meta: bool,
) -> Result<()> {
    let store = StashStore::open()?;

    if meta {
        let row = store.resolve(handle)?;
        crate::cmds::system::stash_cmd::print_meta(&row, store.handle_len());
        return Ok(());
    }

    let (_row, content) = store.retrieve(handle)?;

    let filtered = apply_filters(&content, lines.as_deref(), grep.as_deref(), head, tail)?;
    print!("{filtered}");
    if !filtered.ends_with('\n') && !filtered.is_empty() {
        println!();
    }
    Ok(())
}

fn apply_filters(
    content: &str,
    lines: Option<&str>,
    grep: Option<&str>,
    head: Option<usize>,
    tail: Option<usize>,
) -> Result<String> {
    let mut current: Vec<&str> = content.lines().collect();

    if let Some(pattern) = grep {
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow!("invalid --grep pattern '{pattern}': {e}"))?;
        current.retain(|l| re.is_match(l));
    }

    if let Some(spec) = lines {
        let (start, end) = parse_line_range(spec, current.len())?;
        // 1-indexed inclusive → slice bounds.
        let lo = start.saturating_sub(1).min(current.len());
        let hi = end.min(current.len());
        current = if lo < hi { current[lo..hi].to_vec() } else { Vec::new() };
    }

    if let Some(n) = head {
        current.truncate(n);
    }

    if let Some(n) = tail {
        if current.len() > n {
            current = current[current.len() - n..].to_vec();
        }
    }

    Ok(current.join("\n"))
}

/// Parse a 1-indexed inclusive `A-B` / `A-` / `-B` / `A` line spec.
fn parse_line_range(spec: &str, total: usize) -> Result<(usize, usize)> {
    let spec = spec.trim();
    let bad = || anyhow!("invalid --lines range '{spec}' (expected A-B, A-, -B, or A)");
    if let Some((a, b)) = spec.split_once('-') {
        let start = if a.trim().is_empty() {
            1
        } else {
            a.trim().parse::<usize>().map_err(|_| bad())?
        };
        let end = if b.trim().is_empty() {
            total
        } else {
            b.trim().parse::<usize>().map_err(|_| bad())?
        };
        if start == 0 {
            return Err(bad());
        }
        Ok((start, end.max(start)))
    } else {
        let n = spec.parse::<usize>().map_err(|_| bad())?;
        if n == 0 {
            return Err(bad());
        }
        Ok((n, n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "alpha\nbeta\ngamma\ndelta\nepsilon";

    #[test]
    fn grep_filters_matching_lines() {
        let out = apply_filters(SAMPLE, None, Some("a$"), None, None).unwrap();
        assert_eq!(out, "alpha\nbeta\ngamma\ndelta");
    }

    #[test]
    fn lines_range_inclusive() {
        let out = apply_filters(SAMPLE, Some("2-4"), None, None, None).unwrap();
        assert_eq!(out, "beta\ngamma\ndelta");
    }

    #[test]
    fn lines_open_ended() {
        assert_eq!(
            apply_filters(SAMPLE, Some("4-"), None, None, None).unwrap(),
            "delta\nepsilon"
        );
        assert_eq!(
            apply_filters(SAMPLE, Some("-2"), None, None, None).unwrap(),
            "alpha\nbeta"
        );
    }

    #[test]
    fn head_and_tail() {
        assert_eq!(
            apply_filters(SAMPLE, None, None, Some(2), None).unwrap(),
            "alpha\nbeta"
        );
        assert_eq!(
            apply_filters(SAMPLE, None, None, None, Some(2)).unwrap(),
            "delta\nepsilon"
        );
    }

    #[test]
    fn grep_then_head_compose() {
        let out = apply_filters(SAMPLE, None, Some("a"), Some(2), None).unwrap();
        assert_eq!(out, "alpha\nbeta");
    }

    #[test]
    fn single_line_spec() {
        assert_eq!(
            apply_filters(SAMPLE, Some("3"), None, None, None).unwrap(),
            "gamma"
        );
    }

    #[test]
    fn invalid_range_errors() {
        assert!(apply_filters(SAMPLE, Some("0"), None, None, None).is_err());
        assert!(apply_filters(SAMPLE, Some("x-y"), None, None, None).is_err());
    }

    #[test]
    fn invalid_grep_errors() {
        assert!(apply_filters(SAMPLE, None, Some("("), None, None).is_err());
    }
}

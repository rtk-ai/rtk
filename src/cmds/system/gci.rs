//! RTK-native compact filesystem listing.
//!
//! This is intentionally not PowerShell `Get-ChildItem` compatibility.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 100_000;

pub fn parse_depth(value: &str) -> Result<usize, String> {
    parse_positive(value, "max depth", usize::MAX)
}

pub fn parse_limit(value: &str) -> Result<usize, String> {
    parse_positive(value, "limit", MAX_LIMIT)
}

fn parse_positive(value: &str, label: &str, maximum: usize) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{label} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{label} must be greater than zero"));
    }
    if parsed > maximum {
        return Err(format!("{label} must not exceed {maximum}"));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    LinkLike,
}

#[derive(Debug, Eq)]
struct Entry {
    path: PathBuf,
    kind: EntryKind,
    size: u64,
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.path.cmp(&other.path)
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

struct Listing {
    retained: BinaryHeap<Entry>,
    total: usize,
    had_error: bool,
    limit: usize,
}

impl Listing {
    fn new(limit: usize) -> Self {
        Self {
            retained: BinaryHeap::with_capacity(limit),
            total: 0,
            had_error: false,
            limit,
        }
    }

    fn retain(&mut self, entry: Entry) {
        self.total = self.total.saturating_add(1);
        if self.retained.len() < self.limit {
            self.retained.push(entry);
        } else if self.retained.peek().is_some_and(|greatest| entry < *greatest) {
            self.retained.pop();
            self.retained.push(entry);
        }
    }

    fn finish(self) -> (String, bool, usize) {
        let shown = self.retained.len();
        let mut entries = self.retained.into_vec();
        entries.sort_unstable();

        let mut output = String::new();
        for entry in entries {
            match entry.kind {
                EntryKind::Directory => output.push_str("d "),
                EntryKind::File => output.push_str(&format!("f {} ", entry.size)),
                EntryKind::LinkLike => output.push_str("l "),
            }
            output.push_str(&entry.path.to_string_lossy());
            output.push('\n');
        }

        if self.total > shown {
            let omitted = self.total - shown;
            if self.total <= MAX_LIMIT {
                output.push_str(&format!(
                    "[shown {shown} of {}; {omitted} omitted; use --limit {}]\n",
                    self.total, self.total
                ));
            } else {
                output.push_str(&format!(
                    "[shown {shown} of {}; {omitted} omitted; --limit maximum {MAX_LIMIT}]\n",
                    self.total
                ));
            }
        }

        (output, self.had_error, self.total)
    }
}

pub fn run(
    roots: &[PathBuf],
    all: bool,
    recursive: bool,
    max_depth: Option<usize>,
    filter: Option<&str>,
    limit: usize,
    verbose: u8,
) -> i32 {
    let mut report = |path: &Path, error: &std::io::Error| {
        eprintln!("rtk gci: {}: {error}", path.display());
    };
    let (output, code, total) = execute(
        roots,
        all,
        recursive,
        max_depth,
        filter,
        limit,
        &mut report,
    );
    print!("{output}");
    if verbose > 0 {
        eprintln!("gci: {total} matching entries");
    }
    code
}

fn execute(
    roots: &[PathBuf],
    all: bool,
    recursive: bool,
    max_depth: Option<usize>,
    filter: Option<&str>,
    limit: usize,
    report: &mut dyn FnMut(&Path, &std::io::Error),
) -> (String, i32, usize) {
    let default_root = PathBuf::from(".");
    let roots = if roots.is_empty() {
        std::slice::from_ref(&default_root)
    } else {
        roots
    };
    let mut listing = Listing::new(limit);

    for root in roots {
        enumerate_root(
            root,
            all,
            recursive,
            max_depth,
            filter,
            &mut listing,
            report,
        );
    }

    let (output, had_error, total) = listing.finish();
    (output, if had_error { 1 } else { 0 }, total)
}

fn enumerate_root(
    root: &Path,
    all: bool,
    recursive: bool,
    max_depth: Option<usize>,
    filter: Option<&str>,
    listing: &mut Listing,
    report: &mut dyn FnMut(&Path, &std::io::Error),
) {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) => {
            report_error(root, &error, listing, report);
            return;
        }
    };

    let root_kind = entry_kind(&metadata);
    if root_kind != EntryKind::Directory {
        consider_entry(root.to_path_buf(), metadata, all, filter, listing);
        return;
    }

    let mut pending = vec![(root.to_path_buf(), 0usize)];
    while let Some((directory, parent_depth)) = pending.pop() {
        let read_dir = match fs::read_dir(&directory) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                report_error(&directory, &error, listing, report);
                continue;
            }
        };

        for child in read_dir {
            let child = match child {
                Ok(child) => child,
                Err(error) => {
                    report_error(&directory, &error, listing, report);
                    continue;
                }
            };
            let path = child.path();
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report_error(&path, &error, listing, report);
                    continue;
                }
            };
            let depth = parent_depth.saturating_add(1);
            let kind = entry_kind(&metadata);
            let hidden = is_hidden(&path, &metadata);

            if !hidden || all {
                consider_entry(path.clone(), metadata, true, filter, listing);
            }

            let below_depth_limit = max_depth.is_none_or(|maximum| depth < maximum);
            if recursive
                && below_depth_limit
                && kind == EntryKind::Directory
                && (!hidden || all)
            {
                pending.push((path, depth));
            }
        }
    }
}

fn consider_entry(
    path: PathBuf,
    metadata: fs::Metadata,
    all: bool,
    filter: Option<&str>,
    listing: &mut Listing,
) {
    if !all && is_hidden(&path, &metadata) {
        return;
    }
    let Some(name) = path.file_name() else {
        return;
    };
    let name = name.to_string_lossy();
    if filter.is_some_and(|pattern| !glob_match(pattern, &name)) {
        return;
    }
    let kind = entry_kind(&metadata);
    listing.retain(Entry {
        path,
        kind,
        size: if kind == EntryKind::File {
            metadata.len()
        } else {
            0
        },
    });
}

fn entry_kind(metadata: &fs::Metadata) -> EntryKind {
    if is_link_like(metadata) {
        EntryKind::LinkLike
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else if metadata.is_file() {
        EntryKind::File
    } else {
        EntryKind::LinkLike
    }
}

fn is_link_like(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn is_hidden(path: &Path, metadata: &fs::Metadata) -> bool {
    let dot_prefixed = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'));
    if dot_prefixed {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
        metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn report_error(
    path: &Path,
    error: &std::io::Error,
    listing: &mut Listing,
    report: &mut dyn FnMut(&Path, &std::io::Error),
) {
    report(path, error);
    listing.had_error = true;
}

fn glob_match(pattern: &str, name: &str) -> bool {
    #[cfg(windows)]
    let (pattern, name) = (pattern.to_lowercase(), name.to_lowercase());
    #[cfg(not(windows))]
    let (pattern, name) = (pattern.to_owned(), name.to_owned());

    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n, mut star, mut retry) = (0usize, 0usize, None, 0usize);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = n;
        } else if let Some(star_index) = star {
            p = star_index + 1;
            retry += 1;
            n = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn file(path: &Path, contents: &[u8]) {
        let mut handle = fs::File::create(path).unwrap();
        handle.write_all(contents).unwrap();
    }

    fn collect(
        roots: &[PathBuf],
        all: bool,
        recursive: bool,
        max_depth: Option<usize>,
        filter: Option<&str>,
        limit: usize,
    ) -> (String, bool, usize) {
        let mut listing = Listing::new(limit);
        let mut report = |_: &Path, _: &std::io::Error| {};
        for root in roots {
            enumerate_root(
                root,
                all,
                recursive,
                max_depth,
                filter,
                &mut listing,
                &mut report,
            );
        }
        listing.finish()
    }

    #[test]
    fn non_recursive_lists_types_sizes_spaces_and_unicode() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("nested dir")).unwrap();
        file(&temp.path().join("hello world.txt"), b"four");
        file(&temp.path().join("ោអ.txt"), b"123");

        let (output, error, total) = collect(
            &[temp.path().to_path_buf()],
            false,
            false,
            None,
            None,
            200,
        );
        assert!(!error);
        assert_eq!(total, 3);
        assert!(output.contains("d "));
        assert!(output.contains("f 4 "));
        assert!(output.contains("hello world.txt"));
        assert!(output.contains("ោអ.txt"));
    }

    #[test]
    fn recursive_depth_boundary_and_filter_do_not_prune() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("does-not-match");
        fs::create_dir(&nested).unwrap();
        file(&nested.join("match.txt"), b"x");
        fs::create_dir(nested.join("deeper")).unwrap();
        file(&nested.join("deeper").join("match.txt"), b"xx");

        let (output, error, total) = collect(
            &[temp.path().to_path_buf()],
            false,
            true,
            Some(2),
            Some("*.txt"),
            200,
        );
        assert!(!error);
        assert_eq!(total, 1);
        assert!(output.contains(&nested.join("match.txt").to_string_lossy().to_string()));
        assert!(!output.contains(&nested.join("deeper").join("match.txt").to_string_lossy().to_string()));
    }

    #[test]
    fn file_root_empty_directory_multiple_roots_and_missing_root() {
        let temp = TempDir::new().unwrap();
        let empty = temp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let one = temp.path().join("one.txt");
        file(&one, b"1");
        let missing = temp.path().join("missing");

        let (output, error, total) = collect(&[one.clone(), empty, missing], false, false, None, None, 200);
        assert!(error);
        assert_eq!(total, 1);
        assert!(output.contains(&one.to_string_lossy().to_string()));
    }

    #[test]
    fn partial_root_failure_returns_nonzero_and_reports_failed_root() {
        let temp = TempDir::new().unwrap();
        let good = temp.path().join("good.txt");
        file(&good, b"1");
        let missing = temp.path().join("missing");
        let mut diagnostics = Vec::new();
        let mut report = |path: &Path, error: &std::io::Error| {
            diagnostics.push(format!("{}: {error}", path.display()));
        };

        let (output, code, total) = execute(
            &[good.clone(), missing.clone()],
            false,
            false,
            None,
            None,
            200,
            &mut report,
        );
        assert_eq!(code, 1);
        assert_eq!(total, 1);
        assert!(output.contains(&good.to_string_lossy().to_string()));
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].contains(&missing.to_string_lossy().to_string()));
    }

    #[test]
    fn dotfiles_and_gitignore_are_only_hidden_by_name() {
        let temp = TempDir::new().unwrap();
        file(&temp.path().join(".hidden"), b"x");
        file(&temp.path().join(".gitignore"), b"ignored.txt\n");
        file(&temp.path().join("ignored.txt"), b"y");

        let root = [temp.path().to_path_buf()];
        let (normal, _, normal_total) = collect(&root, false, false, None, None, 200);
        assert_eq!(normal_total, 1);
        assert!(normal.contains("ignored.txt"));
        let (all, _, all_total) = collect(&root, true, false, None, None, 200);
        assert_eq!(all_total, 3);
        assert!(all.contains(".hidden"));
    }

    #[test]
    fn explicitly_supplied_hidden_roots_follow_root_type_semantics() {
        let temp = TempDir::new().unwrap();
        let hidden_file = temp.path().join(".hidden-file");
        file(&hidden_file, b"x");
        let hidden_dir = temp.path().join(".hidden-dir");
        fs::create_dir(&hidden_dir).unwrap();
        file(&hidden_dir.join("visible.txt"), b"y");
        file(&hidden_dir.join(".hidden-child"), b"z");

        let (file_default, _, file_default_total) =
            collect(std::slice::from_ref(&hidden_file), false, false, None, None, 200);
        assert_eq!(file_default_total, 0);
        assert!(file_default.is_empty());
        let (file_all, _, file_all_total) =
            collect(std::slice::from_ref(&hidden_file), true, false, None, None, 200);
        assert_eq!(file_all_total, 1);
        assert!(file_all.contains(".hidden-file"));

        let (dir_default, _, dir_default_total) =
            collect(std::slice::from_ref(&hidden_dir), false, false, None, None, 200);
        assert_eq!(dir_default_total, 1);
        assert!(dir_default.contains("visible.txt"));
        assert!(!dir_default.contains(".hidden-child"));
        let (dir_all, _, dir_all_total) =
            collect(std::slice::from_ref(&hidden_dir), true, false, None, None, 200);
        assert_eq!(dir_all_total, 2);
        assert!(dir_all.contains("visible.txt"));
        assert!(dir_all.contains(".hidden-child"));
    }

    #[test]
    fn deterministic_top_n_has_honest_marker_and_bounded_retention() {
        let temp = TempDir::new().unwrap();
        for name in ["z.txt", "a.txt", "m.txt", "b.txt"] {
            file(&temp.path().join(name), b"x");
        }
        let root = [temp.path().to_path_buf()];
        let (first, _, total) = collect(&root, false, false, None, None, 2);
        let (second, _, _) = collect(&root, false, false, None, None, 2);
        assert_eq!(first, second);
        assert_eq!(total, 4);
        assert!(first.contains("a.txt"));
        assert!(first.contains("b.txt"));
        assert!(!first.contains("m.txt"));
        assert!(first.contains("[shown 2 of 4; 2 omitted; use --limit 4]"));
    }

    #[test]
    fn glob_grammar_is_unicode_scalar_aware() {
        assert!(glob_match("?.txt", "ោ.txt"));
        assert!(glob_match("a*c", "abbbc"));
        assert!(!glob_match("a?c", "ac"));
    }

    #[test]
    fn numeric_parsers_reject_zero_and_excessive_limits() {
        assert_eq!(parse_depth("1"), Ok(1));
        assert!(parse_depth("0").is_err());
        assert_eq!(parse_limit("200"), Ok(200));
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("100001").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_directory_is_listed_but_never_traversed() {
        use std::os::unix::fs::symlink;
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        file(&target.join("inside.txt"), b"x");
        let link = temp.path().join("link");
        symlink(&target, &link).unwrap();

        let (output, error, _) = collect(std::slice::from_ref(&link), false, true, None, None, 200);
        assert!(!error);
        assert_eq!(output, format!("l {}\n", link.display()));
    }

    #[cfg(windows)]
    #[test]
    fn directory_symlink_is_listed_but_never_traversed_when_available() {
        use std::os::windows::fs::symlink_dir;
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        file(&target.join("inside.txt"), b"x");
        let link = temp.path().join("link");
        if let Err(error) = symlink_dir(&target, &link) {
            eprintln!("skipping Windows symlink test: {error}");
            return;
        }

        let (output, error, _) = collect(std::slice::from_ref(&link), false, true, None, None, 200);
        assert!(!error);
        assert_eq!(output, format!("l {}\n", link.display()));
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_listed_but_never_traversed_when_available() {
        use std::process::Command;
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        file(&target.join("inside.txt"), b"x");
        let junction = temp.path().join("junction");
        let status = Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .status()
            .unwrap();
        if !status.success() {
            eprintln!("skipping Windows junction test: mklink /J unavailable");
            return;
        }

        let (output, error, _) = collect(
            std::slice::from_ref(&junction),
            false,
            true,
            None,
            None,
            200,
        );
        assert!(!error);
        assert_eq!(output, format!("l {}\n", junction.display()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_hidden_attribute_controls_listing_and_traversal() {
        use std::process::Command;
        let temp = TempDir::new().unwrap();
        let hidden_file = temp.path().join("hidden.txt");
        file(&hidden_file, b"x");
        let hidden_dir = temp.path().join("hidden-dir");
        fs::create_dir(&hidden_dir).unwrap();
        file(&hidden_dir.join("inside.txt"), b"y");
        for path in [&hidden_file, &hidden_dir] {
            let status = Command::new("attrib").arg("+h").arg(path).status().unwrap();
            assert!(status.success(), "attrib +h failed for {}", path.display());
        }

        let root = [temp.path().to_path_buf()];
        let (normal, error, total) = collect(&root, false, true, None, None, 200);
        assert!(!error);
        assert_eq!(total, 0);
        assert!(normal.is_empty());

        let (all, error, total) = collect(&root, true, true, None, None, 200);
        assert!(!error);
        assert_eq!(total, 3);
        assert!(all.contains("hidden.txt"));
        assert!(all.contains("hidden-dir"));
        assert!(all.contains("inside.txt"));

        for path in [&hidden_file, &hidden_dir] {
            let _ = Command::new("attrib").arg("-h").arg(path).status();
        }
    }
}

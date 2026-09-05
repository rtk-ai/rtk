//! Compares two files and shows only the changed lines.

use crate::core::guard::never_worse;
use crate::core::tracking;
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

const IDENTICAL_FILES_MESSAGE: &str = "[ok] Files are identical\n";

/// Ultra-condensed diff - only changed lines, no context.
/// Returns the diff-convention exit code: 0 if identical, 1 if files differ.
pub fn run(file1: &Path, file2: &Path, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Comparing: {} vs {}", file1.display(), file2.display());
    }

    let content1 = fs::read_to_string(file1)?;
    let content2 = fs::read_to_string(file2)?;
    let both_files = format!("{}\n---\n{}", content1, content2);

    let comparison = compare_files(&content1, &content2);
    let fallback = classic_fallback(&comparison);
    let (rtk, exit_code) = render_diff(file1, file2, &comparison);
    let shown = select_file_diff_output(&comparison, &fallback, &both_files, &rtk);
    print!("{}", shown);
    timer.track(
        &format!("diff {} {}", file1.display(), file2.display()),
        "rtk diff",
        tracking_baseline(&fallback, &both_files, shown),
        shown,
    );
    Ok(exit_code)
}

/// What comparing the two files established, before anything is rendered.
///
/// The discriminator matters more than it looks. An empty change list is not a
/// synonym for "identical": it is also what a difference `str::lines()` cannot
/// see produces, and what every refusal to build an over-budget listing
/// produces. Routing any of those through the identical branch reports two
/// different files as the same and exits 0, which is the bug this module exists
/// to close.
enum FileComparison {
    /// Byte-identical.
    Identical,
    /// The bytes differ but `lines()` does not. Carries the description of the
    /// cause, since only the file contents can supply it.
    InvisibleDifference(String),
    /// A line-level comparison ran. Either it produced a change list, or it
    /// refused to build one and said why in `DiffResult::unaligned`.
    Lines(DiffResult),
}

fn compare_files(content1: &str, content2: &str) -> FileComparison {
    // Byte equality is the only safe basis for claiming identity, and it must be
    // checked before `lines()` touches the input. `str::lines()` strips a
    // trailing `\r` and treats the final newline as optional, so a CRLF-vs-LF or
    // missing-trailing-newline difference collapses to identical line vectors.
    // Reporting "identical" with exit 0 for files that differ silently passes
    // any verification gate built on `diff`.
    if content1 == content2 {
        return FileComparison::Identical;
    }

    let lines1: Vec<&str> = content1.lines().collect();
    let lines2: Vec<&str> = content2.lines().collect();
    let diff = compute_diff(&lines1, &lines2);

    if diff.unaligned.is_none() && diff.hunks.is_empty() {
        // Bytes differ, lines don't: the difference is exactly what `lines()`
        // normalizes away. Name it rather than rendering an empty change list.
        return FileComparison::InvisibleDifference(describe_invisible_difference(
            content1, content2,
        ));
    }

    FileComparison::Lines(diff)
}

/// What `diff` itself would have printed, or the empty string when there is no
/// such output to compare against.
///
/// A classic diff exists exactly when a change list was built. Identical files
/// make `diff` print nothing, an invisible difference has no lines to list, and
/// a refused listing has no changes to format — for those three the baseline
/// and the guard both fall back to the dump of both files.
fn classic_fallback(comparison: &FileComparison) -> String {
    match comparison {
        FileComparison::Lines(diff) if diff.unaligned.is_none() => format_classic_diff(diff),
        _ => String::new(),
    }
}

/// Baseline the savings are measured against: what `diff` itself would have
/// printed, so the recorded ratio compares like with like and can never go
/// negative -- the guard already caps the shown output at the fallback.
fn tracking_baseline<'a>(fallback: &'a str, both_files: &'a str, shown: &'a str) -> &'a str {
    if !fallback.is_empty() {
        return fallback;
    }

    // No classic diff to measure against, so the dump of both files stands in
    // as the output that would otherwise have to be read. Two near-empty files
    // can make that dump cheaper than the verdict line, which would book a loss
    // against the cheapest possible answer.
    if tracking::estimate_tokens(both_files) >= tracking::estimate_tokens(shown) {
        both_files
    } else {
        shown
    }
}

fn select_file_diff_output<'a>(
    comparison: &FileComparison,
    fallback: &'a str,
    both_files: &'a str,
    rendered: &'a str,
) -> &'a str {
    match comparison {
        // `diff` prints nothing here, so there is no raw output to be worse
        // than: the verdict line is the whole answer.
        FileComparison::Identical => rendered,
        // The raw fallback here is two blobs that look the same, which is the
        // outcome the message exists to prevent, so fewer bytes does not make
        // it the better answer. Shown unconditionally: the message competes
        // with no change list — it fires only when the bytes differ and
        // `lines()` cannot see it — and it has a floor of its own that a fixed
        // allowance above raw cannot clear for a one-line pair. `guard.rs`
        // names this exception.
        FileComparison::InvisibleDifference(_) => rendered,
        // No change list means no classic diff, so the refusal is measured
        // against the dump it is refusing to replace.
        FileComparison::Lines(diff) if diff.unaligned.is_some() => {
            never_worse(both_files, rendered)
        }
        FileComparison::Lines(_) => never_worse(fallback, rendered),
    }
}

/// Renders the condensed file comparison and returns it with the
/// diff-convention exit code (0 = identical, 1 = differences found).
fn render_diff(file1: &Path, file2: &Path, comparison: &FileComparison) -> (String, i32) {
    let diff = match comparison {
        FileComparison::Identical => return (IDENTICAL_FILES_MESSAGE.to_string(), 0),
        FileComparison::InvisibleDifference(cause) => return (format!("{cause}\n"), 1),
        FileComparison::Lines(diff) => diff,
    };

    match &diff.unaligned {
        Some(Unaligned::DifferingLines(n)) => {
            return (
                format!(
                    "{} lines differ, too many to list; use `rtk proxy diff` for the full text\n",
                    n
                ),
                1,
            );
        }
        Some(Unaligned::RegionBounds {
            differing_floor,
            first,
            last1,
            last2,
        }) => {
            // The floor is measured, not derived from the constant: every script
            // shorter than the round the aligner gave up at was tried and
            // failed, and an in-place rewrite is two operations. Where the
            // differences sit is stated as line bounds in each file, because the
            // size of that region is not a count of anything and a figure shaped
            // like one invites reading it as the amount of change.
            return (
                format!(
                    "at least {} lines differ, too different to align line by line\ndifferences fall between lines {}-{} of {} and {}-{} of {}; use `rtk proxy diff` for the full text\n",
                    differing_floor,
                    first,
                    last1,
                    file1.display(),
                    first,
                    last2,
                    file2.display()
                ),
                1,
            );
        }
        Some(Unaligned::EditScript { removed, added }) => {
            // Neither count opens the line: a leading `-` or `+` would read as
            // a listed line to anything anchoring on the markers.
            return (
                format!(
                    "only in {}: {} lines, only in {}: {} lines; too many to list, use `rtk proxy diff` for the full text\n",
                    file1.display(),
                    removed,
                    file2.display(),
                    added
                ),
                1,
            );
        }
        None => {}
    }

    // No `file1 -> file2` header and no blank line: the caller typed both
    // paths, and on an agent-sized diff the framing cost more than the change
    // list saved against `diff`. The counts line is owed only once the listing
    // is long enough to need a summary; below that it is the whole margin.
    // It keeps its indent so it cannot be mistaken for a listed line.
    let listing = format_diff_changes(diff);
    let mut rtk = String::new();
    if listing.lines().count() >= COUNTS_MIN_LISTED_LINES {
        rtk.push_str(&format!(
            "   +{} added, -{} removed, ~{} modified\n",
            diff.added, diff.removed, diff.modified
        ));
    }
    if diff.positional {
        // The pairing is by line position, not by alignment. Saying so is the
        // difference between a non-minimal diff and a misleading one.
        rtk.push_str("   paired by line position: too different to align\n");
    }
    if let Some(legend) = frame_legend(diff) {
        rtk.push_str(&legend);
    }
    rtk.push_str(&listing);
    (rtk, 1)
}

/// Listed lines from which the render opens with the counts line.
///
/// The condensed body beats classic `diff` by a few bytes per isolated edit,
/// so a 37-byte summary erases that margin on any diff of a dozen changes or
/// fewer — the size an agent produces all day. A screenful is where a reader
/// stops counting markers and a summary starts saving a scan, and there the
/// line is a small fraction of the listing.
const COUNTS_MIN_LISTED_LINES: usize = 20;

/// The note explaining which file each marker is numbered in, or `None` when
/// the output has only one frame and needs no note.
///
/// Every line is numbered in the file it comes from: `-` and `~` in file1, `+`
/// in file2. The note is owed whenever the output mixes frames, which is any
/// time a `+` sits beside a `-` or a `~` — an insertion above a modification
/// shifts the numbering just as much as a replacement pair does. Output drawn
/// from one file only (`+` alone, `-` alone, `~` alone) has one frame.
///
/// It names the markers actually on screen rather than a fixed `-` and `+`.
/// The note exists solely to stop a line-number misread, so one that describes
/// output the reader is not looking at — announcing a `-` frame with no lines
/// in it while saying nothing about the `~` lines that are there — is worse
/// than none.
///
/// It names the files by argument position rather than by path. The caller
/// typed both paths, and with them in it the note was the most expensive line
/// on the screen: two fixture paths cost more than the change list saved
/// against `diff`, which is the same reason the render has no header.
///
/// The positional fallback numbers both halves of a pair from the same
/// position, so there is one frame there and the note would misdescribe it as
/// two.
fn frame_legend(diff: &DiffResult) -> Option<String> {
    if diff.positional || diff.added == 0 || (diff.removed == 0 && diff.modified == 0) {
        return None;
    }
    let mut file1_markers = Vec::new();
    if diff.removed > 0 {
        file1_markers.push("-");
    }
    if diff.modified > 0 {
        file1_markers.push("~");
    }
    Some(format!(
        "   ({} = file 1; + = file 2)\n",
        file1_markers.join(",")
    ))
}

/// 1-based numbers of the lines that `content` terminates with CRLF.
///
/// Positions, not a count: two files can hold the same number of CRLF
/// terminators at different lines, and a count alone cannot tell them apart.
fn crlf_line_numbers(content: &str) -> Vec<usize> {
    // Only the newline-terminated part has line terminators to classify.
    // `split` hands back an unterminated tail as its own segment, so a file
    // ending in a bare `\r` would otherwise count a CRLF that isn't there.
    let terminated = match content.rfind('\n') {
        Some(i) => &content[..=i],
        None => "",
    };
    terminated
        .split('\n')
        .enumerate()
        .filter(|(_, segment)| segment.ends_with('\r'))
        .map(|(i, _)| i + 1)
        .collect()
}

/// Describe a difference that a line-based diff cannot see.
///
/// Reached only when the bytes differ but `lines()` yields identical vectors,
/// which narrows the cause to the two things `lines()` normalizes: a `\r`
/// before the newline, and the presence of the final newline. Rendering the
/// usual `~ 12 foo → foo` change list here would show two visually identical
/// strings, so state the cause instead.
///
/// Returns the cause alone; the caller supplies the `file1 -> file2` header.
fn describe_invisible_difference(content1: &str, content2: &str) -> String {
    let crlf1 = crlf_line_numbers(content1);
    let crlf2 = crlf_line_numbers(content2);
    let nl1 = content1.ends_with('\n');
    let nl2 = content2.ends_with('\n');

    let mut notes: Vec<String> = Vec::new();
    if crlf1.len() != crlf2.len() {
        notes.push(format!(
            "line endings: {} CRLF vs {} CRLF",
            crlf1.len(),
            crlf2.len()
        ));
    } else if crlf1 != crlf2 {
        // Equal counts, different placement. Printing the counts alone would
        // show the same number on both sides, which reads as "no difference".
        let line = crlf1
            .iter()
            .zip(&crlf2)
            .find(|(l1, l2)| l1 != l2)
            .map(|(l1, l2)| (*l1).min(*l2))
            .unwrap_or(0);
        notes.push(format!(
            "line endings: {} CRLF on each side, first differing at line {}",
            crlf1.len(),
            line
        ));
    }
    if nl1 != nl2 {
        notes.push(format!(
            "trailing newline: {} vs {}",
            if nl1 { "present" } else { "absent" },
            if nl2 { "present" } else { "absent" }
        ));
    }
    if notes.is_empty() {
        // Defensive: `lines()` normalizes only the `\r` before a newline and
        // the final newline, so one of the checks above should have fired.
        notes.push("cause outside the line-ending and trailing-newline checks".to_string());
    }

    // Opens with develop's wording so the phrase callers and tests match on
    // survives, then names the measured cause instead of stopping at "no
    // line-content change". Deliberately avoids the word "identical": that
    // string is the signal for the true-identity case, and a reader (or grep)
    // scanning for it must not match a report about files that differ.
    format!(
        "files differ only in whitespace or line endings ({})",
        notes.join("; ")
    )
}

/// Run diff from stdin (piped command output)
pub fn run_stdin(_verbose: u8) -> Result<()> {
    use std::io::{self, Read};
    let timer = tracking::TimedExecution::start();

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    // Parse unified diff format
    let condensed = condense_unified_diff(&input);
    let shown = never_worse(&input, &condensed);
    println!("{}", shown);

    timer.track("diff (stdin)", "rtk diff (stdin)", &input, shown);

    Ok(())
}

/// One maximal run of differing lines.
///
/// `old` is a contiguous range of file1 starting at `start1`, `new` a
/// contiguous range of file2 starting at `start2`; one side may be empty but
/// not both. When a side is empty its start is the line the other side's run
/// would sit before, so `start - 1` is the anchor classic `diff` prints for an
/// append or a delete.
///
/// The lines on either side of a hunk are matched, which is what makes it the
/// unit both renderers group by. Classic `diff` prints one hunk per region,
/// and its format has no way to express which old line became which new line,
/// so rendering a pairing there asserted something the format cannot carry.
/// Only the condensed render reads `pairs`, and the hunk's bounds do not move
/// whichever pairing it chooses.
struct Hunk {
    start1: usize,
    start2: usize,
    old: Vec<String>,
    new: Vec<String>,
    /// `(old index, new index)` for the lines that read as one rewritten line:
    /// similar enough to show as a single `~`. Sorted by old index. Every
    /// unpaired line lists as a `-` or a `+`.
    pairs: Vec<(usize, usize)>,
}

impl Hunk {
    fn at(start1: usize, start2: usize) -> Self {
        Self {
            start1,
            start2,
            old: Vec::new(),
            new: Vec::new(),
            pairs: Vec::new(),
        }
    }

    /// The condensed listing of this hunk: `~` and `-` lines in file1 order,
    /// then the unpaired `+` lines in file2 order.
    fn changes(&self) -> Vec<DiffChange<'_>> {
        let mut out = Vec::with_capacity(self.old.len() + self.new.len());
        let mut paired_new = vec![false; self.new.len()];
        for &(_, j) in &self.pairs {
            paired_new[j] = true;
        }

        let mut pairs = self.pairs.iter().peekable();
        for (i, old) in self.old.iter().enumerate() {
            match pairs.peek() {
                Some(&&(x, j)) if x == i => {
                    pairs.next();
                    out.push(DiffChange::Modified {
                        line1: self.start1 + i,
                        old,
                        new: &self.new[j],
                    });
                }
                _ => out.push(DiffChange::Removed {
                    line1: self.start1 + i,
                    text: old,
                }),
            }
        }
        for (j, new) in self.new.iter().enumerate() {
            if !paired_new[j] {
                out.push(DiffChange::Added {
                    line2: self.start2 + j,
                    text: new,
                });
            }
        }
        out
    }
}

/// One listed line of the condensed render, as a view into a hunk.
///
/// Every line is numbered in the file it comes from: `Removed` and `Modified`
/// in file1, `Added` in file2. The render's legend says so whenever the output
/// mixes the two frames.
#[derive(Debug)]
enum DiffChange<'a> {
    /// A line only in file2.
    Added { line2: usize, text: &'a str },
    /// A line only in file1.
    Removed { line1: usize, text: &'a str },
    /// A line rewritten in place, similar enough to show as a single `~` line.
    Modified {
        line1: usize,
        old: &'a str,
        new: &'a str,
    },
}

struct DiffResult {
    added: usize,
    removed: usize,
    modified: usize,
    hunks: Vec<Hunk>,
    /// Set when no change list was produced, either because the pair could not
    /// be aligned or because the list would be too large to be worth building.
    /// `hunks` is empty by design and the counts are zero because none were
    /// computed.
    unaligned: Option<Unaligned>,
    /// Set when the changes were paired by line position rather than aligned,
    /// which happens past `MAX_TRACE_CELLS` on equal-length inputs.
    positional: bool,
}

impl DiffResult {
    /// The condensed listing across every hunk, in file order.
    fn changes(&self) -> Vec<DiffChange<'_>> {
        self.hunks.iter().flat_map(Hunk::changes).collect()
    }

    /// Counts derived from the hunks: a pair is one rewritten line, and every
    /// unpaired line on either side is one removal or one addition.
    fn from_hunks(hunks: Vec<Hunk>, positional: bool) -> Self {
        let (mut added, mut removed, mut modified) = (0usize, 0usize, 0usize);
        for hunk in &hunks {
            modified += hunk.pairs.len();
            removed += hunk.old.len() - hunk.pairs.len();
            added += hunk.new.len() - hunk.pairs.len();
        }
        Self {
            added,
            removed,
            modified,
            hunks,
            unaligned: None,
            positional,
        }
    }
}

fn format_diff_changes(diff: &DiffResult) -> String {
    let mut out = String::new();
    for change in diff.changes() {
        match change {
            DiffChange::Added { line2, text } => out.push_str(&format!("+{:4} {}\n", line2, text)),
            DiffChange::Removed { line1, text } => {
                out.push_str(&format!("-{:4} {}\n", line1, text))
            }
            DiffChange::Modified { line1, old, new } => {
                out.push_str(&format!("~{:4} {} → {}\n", line1, old, new))
            }
        }
    }
    out
}

/// What `diff` itself prints for the same comparison: one `NcM`, `NaM` or
/// `NdM` hunk per changed region, all `<` lines, then `---`, then all `>`
/// lines.
///
/// Grouped by region rather than by the pairing the condensed render chose.
/// Classic format cannot express a pairing, so splitting a region into an
/// `NcM` plus a trailing `NaM` asserted one implicitly, and that assertion is
/// not always supportable. The region bounds alone determine this output,
/// which is also what makes it the right `never_worse` baseline: an inflated
/// fallback is an easier bar for the condensed render to clear.
///
/// Every hunk header names a position in each file, and after an insertion the
/// two files' numbering no longer agrees, so each hunk carries both. Deriving
/// one frame from the other silently mislabels every hunk past the first shift.
fn format_classic_diff(diff: &DiffResult) -> String {
    let mut out = String::new();
    for hunk in &diff.hunks {
        let range1 = || format_line_range(hunk.start1, hunk.start1 + hunk.old.len() - 1);
        let range2 = || format_line_range(hunk.start2, hunk.start2 + hunk.new.len() - 1);
        match (hunk.old.is_empty(), hunk.new.is_empty()) {
            (true, true) => continue,
            (true, false) => out.push_str(&format!("{}a{}\n", hunk.start1 - 1, range2())),
            (false, true) => out.push_str(&format!("{}d{}\n", range1(), hunk.start2 - 1)),
            (false, false) => out.push_str(&format!("{}c{}\n", range1(), range2())),
        }
        for line in &hunk.old {
            out.push_str(&format!("< {}\n", line));
        }
        if !hunk.old.is_empty() && !hunk.new.is_empty() {
            out.push_str("---\n");
        }
        for line in &hunk.new {
            out.push_str(&format!("> {}\n", line));
        }
    }
    out
}

fn format_line_range(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start},{end}")
    }
}

/// Why a pair produced no change list.
#[derive(Debug, PartialEq, Eq)]
enum Unaligned {
    /// The differing lines could be counted exactly — one side is empty, or the
    /// lengths are equal — and there are too many of them to be worth listing.
    DifferingLines(usize),
    /// Unequal lengths past the trace budget. Nothing was counted, so the only
    /// figures stated are a floor and where the differences are.
    ///
    /// `differing_floor` comes from the round the aligner gave up at: every
    /// shorter script failed, so at least that many lines differ. `first` is the
    /// first differing line in both files, `last1` and `last2` the last in each.
    ///
    /// Line bounds, not a magnitude: the region between the first and last
    /// difference is unrelated to how much changed inside it, so a number
    /// shaped like a count would swing by orders of magnitude on the same
    /// amount of change.
    RegionBounds {
        differing_floor: usize,
        first: usize,
        last1: usize,
        last2: usize,
    },
    /// The pair aligned, but the change list it implies is too long or too
    /// large to build. Both sides are stated because the script knows them
    /// exactly and a single figure would not: `removed` lines of file1 have no
    /// counterpart in file2, and `added` lines of file2 none in file1.
    ///
    /// Not the number of listed lines: the runs have not been paired yet, and a
    /// pairing turns two script steps into one `~`. `removed + added` bounds
    /// that count from above, which is what the budget needs.
    EditScript { removed: usize, added: usize },
}

/// One edit-script step, in the trimmed middle's coordinates. Line numbers are
/// not stored: `ops_to_hunks` walks the script with a cursor in each file, and
/// every step advances one or both, so each hunk's position falls out of the
/// walk.
///
/// `Keep` is a run of matched lines, which is what separates one hunk from the
/// next: a deletion at line 1 and an insertion at line 41 must not fold into a
/// change neither file contains. It carries a count rather than one entry per
/// line because it holds no data, and one entry per unchanged line made the
/// script scale with the file rather than with the change — 45MB of `Keep` for
/// a million-line pair with 700 rewrites.
enum Op {
    Del(String),
    Ins(String),
    Keep(usize),
}

/// Cap on the aligner's trace, counted in `i32` cells.
///
/// The trace is the only part of the alignment that grows: round `d` records
/// the furthest-reaching `x` on the diagonals that could still be on an optimal
/// path, and the backtrack walks those records. It is one flat vector, and
/// each round's record is its snapshot plus `TRACE_ROUND_OVERHEAD` cells of
/// bookkeeping, so the count here is the allocation: 1,000,000 cells is 4MB,
/// whatever shape the pair has.
///
/// Counting cells rather than the edit distance is what lets a lopsided pair
/// through. A one-line file against a five-thousand-line one needs `d = 4999`,
/// but at most one deletion is possible, so each round's window is a few
/// diagonals wide and the whole trace is ~25,000 cells. An edit-distance cap
/// refused that pair and reported an alignment it could have produced in one
/// pass as too different to align.
///
/// A pair whose lengths are close gets the full window, which is where the
/// budget is spent: each round costs `d + 4` cells, so the trace is quadratic
/// in the amount of change and 1,000,000 cells stop the search at `d = 1410`.
/// That is ~705 scattered rewritten lines, the worst case for the window;
/// contiguous change narrows it and reaches further. Past the budget equal
/// lengths fall back to a positional comparison, which cannot run out of it,
/// and unequal ones report bounds.
///
/// ~705 changed lines is the number this constant is really choosing, and it is
/// chosen against `POSITIONAL_CHANGE_CAP` rather than in isolation: an aligner
/// that refuses an order of magnitude below what the listing path prints is a
/// cliff, not a budget. A rewritten line is one listed line and an unpaired
/// one is two, so the aligner tops out within a factor of ~3 of the 5,000-line
/// listing cap. Storing only the diagonals the backtrack reads — every other
/// slot, since a round computes one parity and the backtrack reads the other —
/// is what buys half of that reach at no cost in memory.
///
/// The remaining headroom is bounded by RTK's own budgets rather than by the
/// algorithm. 4MB of trace against the 5MB in CLAUDE.md, and the search that
/// fills it costs ~7ms of user time against the 10ms there. Both are spent
/// only by a pair that actually changed that much; a typical diff never
/// allocates a second round. Doubling the constant doubles both and buys 1.41x
/// the reach.
///
/// The cap is on the amount of change, never on the size of the files or on how
/// far apart the edits sit: two lines changed 2000 lines apart cost `d = 4`,
/// whatever the file's length.
const MAX_TRACE_CELLS: usize = 1_000_000;

/// Cells each round adds to the trace besides its snapshot: the first diagonal
/// the snapshot covers, and the snapshot's length so the backtrack can walk
/// records from the end. Stored inline, so a round costs exactly what the
/// budget charges it — a separate `Vec` per round cost 24 bytes of header plus
/// an allocation each, and on a lopsided pair with a three-diagonal window that
/// was 5.8x the cells being counted.
const TRACE_ROUND_OVERHEAD: usize = 2;

/// Diff two line sequences.
///
/// The original implementation compared **positionally** (`lines1[i]` vs
/// `lines2[i]` for i in 0..max_len), so a single inserted line desynchronized
/// every line after it: each subsequent pair compared unrelated lines, the whole
/// tail rendered as changed, and the output grew large enough that the
/// `never_worse` guard discarded it and dumped both files concatenated instead
/// of showing one insertion.
fn compute_diff(lines1: &[&str], lines2: &[&str]) -> DiffResult {
    // Common prefix and suffix carry no information and dominate real diffs.
    let mut lo = 0usize;
    while lo < lines1.len() && lo < lines2.len() && lines1[lo] == lines2[lo] {
        lo += 1;
    }
    let mut hi1 = lines1.len();
    let mut hi2 = lines2.len();
    while hi1 > lo && hi2 > lo && lines1[hi1 - 1] == lines2[hi2 - 1] {
        hi1 -= 1;
        hi2 -= 1;
    }

    let a = &lines1[lo..hi1];
    let b = &lines2[lo..hi2];

    if a.is_empty() && b.is_empty() {
        return DiffResult::from_hunks(Vec::new(), false);
    }

    // A pure insertion or deletion needs no search: after trimming, one side is
    // empty and the script is a single run. Myers would spend the whole trace
    // budget reaching `(n, m)` here, so without this an appended chunk — or an
    // empty file against a populated one — reports as too different to align.
    if a.is_empty() || b.is_empty() {
        if let Some(refused) = too_much_to_list(a.iter().chain(b.iter()).map(|l| l.len())) {
            return refused;
        }
        return DiffResult::from_hunks(vec![one_sided_hunk(a, b, lo)], false);
    }

    let gave_up_at = match myers_ops(a, b) {
        Ok(Aligned::Script(ops)) => {
            if let Some(refused) = script_too_large(&ops) {
                return refused;
            }
            return ops_to_hunks(ops, lo);
        }
        Ok(Aligned::TooManySteps { removed, added }) => {
            return unaligned(Unaligned::EditScript { removed, added });
        }
        Err(d) => d,
    };

    if a.len() == b.len() {
        // Too much change for an alignment, but equal lengths make pairing by
        // position a valid edit script: it reconstructs file2 and every line
        // number names the text it claims. It is not minimal — a deletion at
        // the top and an insertion at the bottom keep the lengths equal and
        // report every line between them as rewritten — so the render says the
        // pairing is positional rather than presenting it as an alignment.
        //
        // Counting first costs one pass and no allocation, which is what keeps
        // two wholly different 100,000-line files from building a 200,000-line
        // change list nobody asked for.
        let differing = a.iter().zip(b.iter()).filter(|(x, y)| x != y);
        if let Some(refused) = too_much_to_list(differing.map(|(x, y)| x.len() + y.len())) {
            return refused;
        }
        return positional_hunks(a, b, lo);
    }

    unaligned(Unaligned::RegionBounds {
        differing_floor: gave_up_at.div_ceil(2),
        first: lo + 1,
        last1: hi1,
        last2: hi2,
    })
}

/// A result carrying no change list, only the reason there is none. The counts
/// are zero because none were computed, not because nothing changed.
fn unaligned(reason: Unaligned) -> DiffResult {
    DiffResult {
        added: 0,
        removed: 0,
        modified: 0,
        hunks: Vec::new(),
        unaligned: Some(reason),
        positional: false,
    }
}

/// Refuse to build a change list that is too long or too large, naming the
/// exact number of differing lines instead.
///
/// `sizes` yields one entry per differing position the list would report,
/// holding the bytes that position contributes — both halves of a replacement,
/// since one carries the old text and the new. Both callers know the count
/// exactly — one side is empty, or the two sides are the same length — so the
/// refusal states a measured number rather than a bound. The third listing
/// path, an aligned edit script, is guarded by `script_too_large`.
fn too_much_to_list(sizes: impl Iterator<Item = usize>) -> Option<DiffResult> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for size in sizes {
        count += 1;
        bytes += size;
    }
    if !over_listing_budget(count, bytes) {
        return None;
    }
    Some(unaligned(Unaligned::DifferingLines(count)))
}

/// Whether a change list naming `count` differing positions and holding `bytes`
/// bytes of text is too large to be worth building.
fn over_listing_budget(count: usize, bytes: usize) -> bool {
    count > POSITIONAL_CHANGE_CAP || bytes > POSITIONAL_BYTE_CAP
}

/// Refuse an aligned edit script whose change list would hold too many bytes,
/// stating the two counts the script knows exactly instead.
///
/// The third listing path, and the one the band made reachable. While the cap
/// was on the edit distance, a large script meant the aligner had already given
/// up; a banded window keeps the trace cheap while `d` grows, so a one-line file
/// against a 60,000-line one now aligns and would build 59,999 changes — 11x
/// `POSITIONAL_CHANGE_CAP` — from an input the empty-file case refuses at the
/// same size for free.
///
/// The count half of the budget is spent before the script exists: `myers_ops`
/// knows the edit distance the moment it reaches the end, and refuses there
/// rather than materialising a script it is about to throw away. What is left
/// here is the byte half, which genuinely needs the text. `ops` is already
/// built, so this is a pass over what is in hand, and the bytes are exact: each
/// step's text is moved into a hunk and cloned once more into the render.
fn script_too_large(ops: &[Op]) -> Option<DiffResult> {
    let (mut removed, mut added, mut bytes) = (0usize, 0usize, 0usize);
    for op in ops {
        match op {
            Op::Del(text) => {
                removed += 1;
                bytes += text.len();
            }
            Op::Ins(text) => {
                added += 1;
                bytes += text.len();
            }
            Op::Keep(_) => {}
        }
    }
    if !over_listing_budget(removed + added, bytes) {
        return None;
    }
    Some(unaligned(Unaligned::EditScript { removed, added }))
}

/// The single hunk for a pair where one side is empty: every line of the other
/// side, in order. `offset` maps middle-relative indices back to file line
/// numbers after prefix trimming, and is also where the empty side's run would
/// sit — the empty middle consumes nothing, so the whole run follows the
/// trimmed prefix in both files.
fn one_sided_hunk(a: &[&str], b: &[&str], offset: usize) -> Hunk {
    Hunk {
        start1: offset + 1,
        start2: offset + 1,
        old: a.iter().map(|l| (*l).to_string()).collect(),
        new: b.iter().map(|l| (*l).to_string()).collect(),
        pairs: Vec::new(),
    }
}

/// Cap on the differing positions either listing path will name.
///
/// Not a cap on rendered lines: a position whose two texts are too dissimilar
/// to read as one `~` prints a `-` and a `+`, so 5,000 positions can render as
/// up to 10,000 lines. `POSITIONAL_BYTE_CAP` is what bounds the memory; this
/// one bounds how much a reader is asked to scan. Past it the pair is not a
/// diff anyone reads line by line, and the exact count says more than the list
/// would.
const POSITIONAL_CHANGE_CAP: usize = 5_000;

/// Cap on the bytes those positions may hold.
///
/// A count says nothing about line length, and each listed line is cloned into
/// an `Op` and again into the render. Five thousand ten-thousand-character
/// lines are inside the count cap and cost hundreds of megabytes, so the byte
/// budget is what actually bounds the listing.
const POSITIONAL_BYTE_CAP: usize = 2_000_000;

/// Pair line `i` of each side, grouping runs of differing positions into hunks.
/// Only valid when the two sides are the same length, which is what makes every
/// pair a rewrite in place rather than a shift.
fn positional_hunks(a: &[&str], b: &[&str], offset: usize) -> DiffResult {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut open: Option<Hunk> = None;

    for (i, (old, new)) in a.iter().zip(b.iter()).enumerate() {
        if old == new {
            if let Some(hunk) = open.take() {
                hunks.push(hunk);
            }
            continue;
        }
        let hunk = open.get_or_insert_with(|| Hunk::at(offset + i + 1, offset + i + 1));
        if similarity(old, new) > REWRITE_SIMILARITY {
            let index = hunk.old.len();
            hunk.pairs.push((index, index));
        }
        hunk.old.push((*old).to_string());
        hunk.new.push((*new).to_string());
    }
    if let Some(hunk) = open {
        hunks.push(hunk);
    }

    DiffResult::from_hunks(hunks, true)
}

/// What an alignment produced.
enum Aligned {
    /// The edit script, in forward order.
    Script(Vec<Op>),
    /// The pair aligned, but the script has more steps than the listing budget
    /// admits. Both counts are exact without building it: the script has `d`
    /// steps and deletions minus insertions is the length difference.
    TooManySteps { removed: usize, added: usize },
}

/// Myers' greedy edit script: `O((n + m) * d)` in the edit distance `d`, so the
/// cost tracks how much changed rather than how large the files are.
///
/// `Err(d)` when the trace would exceed `MAX_TRACE_CELLS`, carrying the round
/// it gave up at: rounds below it all failed, so the edit distance is at least
/// `d`.
///
/// Cost grows with `d`, but each round still scans the band and runs snakes
/// across the input, so wall time grows with the file length at a fixed `d`.
/// The bound this function keeps is on the trace, not on the total work.
fn myers_ops(a: &[&str], b: &[&str]) -> Result<Aligned, usize> {
    let n = a.len() as i32;
    let m = b.len() as i32;
    // Deleting all of `a` and inserting all of `b` always reaches the target,
    // so the search never needs a longer script than this.
    let max_d = a.len() + b.len();

    // `v[k + vo]` is the furthest `x` reached on diagonal `k = x - y`. `k` runs
    // over `[-m, n]`, and the extra slot on each side absorbs the `k +/- 1`
    // reads at the window edges.
    let vo = m + 1;
    let mut v = vec![0i32; (n + m + 3) as usize];
    // Round `d` records `v` over the diagonals it is about to extend, plus one
    // on each side for what the extension reads, before it runs. That window is
    // what the backtrack walks. One flat vector: each record is the first
    // diagonal it covers, the snapshot, then the snapshot's length, so the
    // backtrack can walk records from the end and the budget below charges
    // exactly the cells stored.
    let mut trace: Vec<i32> = Vec::new();
    let mut rounds = 0usize;

    for d in 0..=max_d {
        let di = d as i32;
        // Only the diagonals that could still be on an optimal path. Reaching
        // `k` at round `d` costs `(d + k) / 2` deletions and `(d - k) / 2`
        // insertions, and neither can exceed the side it consumes. On a lopsided
        // pair that window stays a few diagonals wide however large `d` grows,
        // which is what keeps the trace affordable where an edit-distance cap
        // gave up on a trivial alignment.
        let lo = (-di).max(di - 2 * m);
        let hi = di.min(2 * n - di);
        if hi < lo {
            break;
        }

        // Round `d` computes only the diagonals with `d`'s parity, and the
        // backtrack reads this snapshot only at the opposite parity — the
        // values round `d - 1` left behind, which is what a step back from
        // `k` lands on. Storing every other slot is not a sampling: the ones
        // skipped are the ones round `d` is about to overwrite and nothing
        // ever reads from here.
        let slots = ((hi - lo) / 2 + 2) as usize;
        if trace.len() + slots + TRACE_ROUND_OVERHEAD > MAX_TRACE_CELLS {
            return Err(d);
        }

        trace.push(lo - 1);
        let mut ks = lo - 1;
        while ks <= hi + 1 {
            trace.push(v[(ks + vo) as usize]);
            ks += 2;
        }
        trace.push(slots as i32);
        rounds += 1;

        let mut k = lo;
        while k <= hi {
            let ki = (k + vo) as usize;
            let mut x = if k == -di || (k != di && v[ki - 1] < v[ki + 1]) {
                v[ki + 1]
            } else {
                v[ki - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[ki] = x;
            if x >= n && y >= m {
                // The script has `d` steps, and deletions minus insertions is
                // `n - m`, so both counts are known before a single line is
                // cloned. Refusing here is what keeps a one-line file against
                // 40,000 long ones — a shape the band aligns comfortably — from
                // materialising a script only to throw it away.
                let deletions = ((di + n - m) / 2) as usize;
                let insertions = ((di - n + m) / 2) as usize;
                if deletions + insertions > POSITIONAL_CHANGE_CAP {
                    return Ok(Aligned::TooManySteps {
                        removed: deletions,
                        added: insertions,
                    });
                }
                return Ok(Aligned::Script(myers_backtrack(&trace, rounds, a, b)));
            }
            k += 2;
        }
    }

    Err(max_d)
}

/// Walk the trace back from `(n, m)` to the origin, emitting the edit script in
/// forward order. `rounds` is the number of records in `trace`; the last one
/// belongs to the round that reached the end.
fn myers_backtrack(trace: &[i32], rounds: usize, a: &[&str], b: &[&str]) -> Vec<Op> {
    let mut ops: Vec<Op> = Vec::new();
    let mut x = a.len() as i32;
    let mut y = b.len() as i32;
    let mut end = trace.len();

    for d in (0..rounds).rev() {
        let di = d as i32;
        // Records are read from the end: the last cell is the snapshot's
        // length, the snapshot precedes it, and the cell before that is the
        // first diagonal the snapshot covers.
        let slots = trace[end - 1] as usize;
        let v = &trace[end - 1 - slots..end - 1];
        let base = trace[end - 2 - slots];
        end -= slots + TRACE_ROUND_OVERHEAD;

        let at = |k: i32| -> i32 {
            // `base` is `lo - 1`, and the snapshot steps by two from there, so
            // only diagonals of `base`'s parity are held. Every read below is
            // at that parity; anything else is off the band, where the
            // furthest-reaching `x` is still the initial 0.
            let i = k - base;
            if i < 0 || i % 2 != 0 {
                return 0;
            }
            let i = (i / 2) as usize;
            if i >= v.len() { 0 } else { v[i] }
        };

        let k = x - y;
        let prev_k = if k == -di || (k != di && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;

        // The snake: matched lines between the previous step and this point.
        let keep = (x - prev_x).min(y - prev_y);
        if keep > 0 {
            ops.push(Op::Keep(keep as usize));
            x -= keep;
            y -= keep;
        }
        if d > 0 {
            // A step consumes one side only.
            if x == prev_x {
                ops.push(Op::Ins(b[(y - 1) as usize].to_string()));
            } else {
                ops.push(Op::Del(a[(x - 1) as usize].to_string()));
            }
        }
        x = prev_x;
        y = prev_y;
    }

    ops.reverse();
    ops
}

/// Fold the edit script into hunks: one per maximal run of steps with no
/// matched line between them.
///
/// The deletions in such a run are consecutive file1 lines and the insertions
/// consecutive file2 lines, whatever order the script interleaves them in, so
/// the run's bounds are the hunk's. `Op::Keep` is the separator: a deletion at
/// line 1 and an insertion at line 41 stay in different hunks because the 39
/// matched lines between them are in the script.
///
/// `offset` is the trimmed prefix's length, so the cursors start there and
/// every hunk is numbered in its file rather than in the middle.
fn ops_to_hunks(ops: Vec<Op>, offset: usize) -> DiffResult {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut open: Option<Hunk> = None;
    // Lines of each file consumed so far, so a hunk opens at `cursor + 1` in
    // both files whichever side its first step is on.
    let (mut cursor1, mut cursor2) = (offset, offset);

    for op in ops {
        match op {
            Op::Keep(run) => {
                if let Some(hunk) = open.take() {
                    hunks.push(hunk);
                }
                cursor1 += run;
                cursor2 += run;
            }
            Op::Del(text) => {
                open.get_or_insert_with(|| Hunk::at(cursor1 + 1, cursor2 + 1))
                    .old
                    .push(text);
                cursor1 += 1;
            }
            Op::Ins(text) => {
                open.get_or_insert_with(|| Hunk::at(cursor1 + 1, cursor2 + 1))
                    .new
                    .push(text);
                cursor2 += 1;
            }
        }
    }
    if let Some(hunk) = open {
        hunks.push(hunk);
    }

    for hunk in &mut hunks {
        hunk.pairs = pair_rewrites(&hunk.old, &hunk.new);
    }
    DiffResult::from_hunks(hunks, false)
}

/// Jaccard similarity of two lines' character sets above which they read as
/// one line rewritten rather than one removed and one added.
const REWRITE_SIMILARITY: f64 = 0.5;

/// Largest `old x new` a hunk may have and still be paired by similarity.
///
/// Above it the pairing is positional. An ordinary hunk is a line or two a
/// side, so the bound is not reached by the diffs this render is for; it exists
/// to keep a wholly rewritten block from scoring every line against every
/// other, where position is as good a guess as any.
const PAIRING_CELL_CAP: usize = 256;

/// Which old lines of a hunk were rewritten into which new lines.
///
/// Best similarity first, not position. Pairing `old[p]` with `new[p]` was
/// wrong whenever a run had unequal deletion and insertion counts, because an
/// insertion at the head of the run shifted every pairing after it, and the
/// similarity threshold cannot repair a wrong pair: a rewritten line usually
/// clears 0.5 against the line inserted beside it too, so the positionally
/// first candidate passed. Taking the highest-scoring pairs greedily claims
/// fewer rewrites that never happened at no cost in output size, which depends
/// on how many pairs there are and not on which.
///
/// Greedy rather than an optimal assignment on purpose: maximising the total
/// similarity accepts individually wrong matches to raise the sum. Nearer the
/// diagonal breaks ties — a character-set score cannot separate `value = 9`
/// from `value = 2` as rewrites of `value = 1` — then old-then-new index so the
/// result is deterministic.
fn pair_rewrites(old: &[String], new: &[String]) -> Vec<(usize, usize)> {
    if old.is_empty() || new.is_empty() {
        return Vec::new();
    }
    let old_sets: Vec<HashSet<char>> = old.iter().map(|l| l.chars().collect()).collect();
    let new_sets: Vec<HashSet<char>> = new.iter().map(|l| l.chars().collect()).collect();

    let mut pairs: Vec<(usize, usize)> = Vec::new();
    if old.len() * new.len() > PAIRING_CELL_CAP {
        for (i, (o, n)) in old_sets.iter().zip(&new_sets).enumerate() {
            if jaccard(o, n) > REWRITE_SIMILARITY {
                pairs.push((i, i));
            }
        }
        return pairs;
    }

    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (i, o) in old_sets.iter().enumerate() {
        for (j, n) in new_sets.iter().enumerate() {
            let score = jaccard(o, n);
            if score > REWRITE_SIMILARITY {
                candidates.push((score, i, j));
            }
        }
    }
    candidates.sort_by(|p, q| {
        q.0.total_cmp(&p.0)
            .then_with(|| p.1.abs_diff(p.2).cmp(&q.1.abs_diff(q.2)))
            .then_with(|| (p.1, p.2).cmp(&(q.1, q.2)))
    });

    let mut used_old = vec![false; old.len()];
    let mut used_new = vec![false; new.len()];
    for (_, i, j) in candidates {
        if used_old[i] || used_new[j] {
            continue;
        }
        used_old[i] = true;
        used_new[j] = true;
        pairs.push((i, j));
    }
    pairs.sort_unstable();
    pairs
}

fn similarity(a: &str, b: &str) -> f64 {
    let a_chars: HashSet<char> = a.chars().collect();
    let b_chars: HashSet<char> = b.chars().collect();
    jaccard(&a_chars, &b_chars)
}

/// Jaccard index of two character sets; 1.0 for two empty sets by convention.
fn jaccard(a: &HashSet<char>, b: &HashSet<char>) -> f64 {
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}

fn condense_unified_diff(diff: &str) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut changes = Vec::new();

    // Never truncate diff content — users make decisions based on this data.
    // Only strip diff metadata (headers, @@ hunks); all +/- lines shown in full.
    for line in diff.lines() {
        if line.starts_with("diff --git") || line.starts_with("--- ") || line.starts_with("+++ ") {
            if line.starts_with("+++ ") {
                if !current_file.is_empty() && (added > 0 || removed > 0) {
                    result.push(format!("[file] {} (+{} -{})", current_file, added, removed));
                    // Column 0: anchored greps (`^[+-]`) must match these.
                    result.append(&mut changes);
                }
                current_file = line
                    .trim_start_matches("+++ ")
                    .trim_start_matches("b/")
                    .to_string();
                added = 0;
                removed = 0;
                changes.clear();
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
            changes.push(line.to_string());
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
            changes.push(line.to_string());
        }
    }

    // Last file
    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("[file] {} (+{} -{})", current_file, added, removed));
        // Column 0: anchored greps (`^[+-]`) must match these.
        result.append(&mut changes);
    }

    result.join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Compare two file contents and render the result, which is the path
    /// `run` takes minus the guard and the tracking.
    fn render_file_diff(
        file1: &Path,
        file2: &Path,
        content1: &str,
        content2: &str,
    ) -> (String, i32) {
        render_diff(file1, file2, &compare_files(content1, content2))
    }

    /// The change list `compare_files` produced, or a panic naming what it
    /// produced instead.
    fn changes_of(content1: &str, content2: &str) -> DiffResult {
        match compare_files(content1, content2) {
            FileComparison::Lines(diff) => diff,
            FileComparison::Identical => panic!("expected a change list, files were identical"),
            FileComparison::InvisibleDifference(cause) => {
                panic!("expected a change list, got an invisible difference: {cause}")
            }
        }
    }

    // --- similarity ---

    #[test]
    fn test_similarity_identical() {
        assert_eq!(similarity("hello", "hello"), 1.0);
    }

    #[test]
    fn test_similarity_completely_different() {
        assert_eq!(similarity("abc", "xyz"), 0.0);
    }

    #[test]
    fn test_similarity_empty_strings() {
        // Both empty: union is 0, returns 1.0 by convention
        assert_eq!(similarity("", ""), 1.0);
    }

    #[test]
    fn test_similarity_partial_overlap() {
        let s = similarity("abcd", "abef");
        // Shared: a, b. Union: a, b, c, d, e, f = 6. Jaccard = 2/6
        assert!((s - 2.0 / 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_similarity_threshold_for_modified() {
        // "let x = 1;" vs "let x = 2;" should be > 0.5 (treated as modification)
        assert!(similarity("let x = 1;", "let x = 2;") > 0.5);
    }

    // --- compute_diff ---

    #[test]
    fn test_compute_diff_identical() {
        let a = vec!["line1", "line2", "line3"];
        let b = vec!["line1", "line2", "line3"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert_eq!(result.modified, 0);
        assert!(result.changes().is_empty());
    }

    #[test]
    fn test_compute_diff_added_lines() {
        let a = vec!["line1"];
        let b = vec!["line1", "line2", "line3"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 2);
        assert_eq!(result.removed, 0);
    }

    #[test]
    fn test_compute_diff_removed_lines() {
        let a = vec!["line1", "line2", "line3"];
        let b = vec!["line1"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.removed, 2);
        assert_eq!(result.added, 0);
    }

    #[test]
    fn test_compute_diff_modified_line() {
        // Similar lines (>0.5 similarity) are classified as modified
        let a = vec!["let x = 1;"];
        let b = vec!["let x = 2;"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 1);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
    }

    #[test]
    fn test_compute_diff_completely_different_line() {
        // Dissimilar lines (<= 0.5 similarity) are added+removed, not modified
        let a = vec!["aaaa"];
        let b = vec!["zzzz"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 0);
        assert_eq!(result.added, 1);
        assert_eq!(result.removed, 1);
    }

    #[test]
    fn test_compute_diff_empty_inputs() {
        let result = compute_diff(&[], &[]);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert!(result.changes().is_empty());
    }

    // --- compute_diff: LCS alignment, not positional ---

    #[test]
    fn test_compute_diff_single_insertion_does_not_desync_the_tail() {
        // The bug: positional compare paired a[i] against b[i], so inserting one
        // line at the top made every later pair compare unrelated lines and the
        // whole file rendered as changed.
        let a = vec!["one", "two", "three", "four", "five"];
        let b = vec!["INSERTED", "one", "two", "three", "four", "five"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 1, "exactly one line was added");
        assert_eq!(result.removed, 0, "nothing was removed");
        assert_eq!(result.modified, 0, "nothing was modified");
        assert_eq!(result.changes().len(), 1);
        match &result.changes()[0] {
            DiffChange::Added { line2, text } => {
                assert_eq!(*text, "INSERTED");
                assert_eq!(*line2, 1);
            }
            other => panic!("expected a single Added, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_diff_insertion_in_the_middle() {
        let a = vec!["a", "b", "c", "d"];
        let b = vec!["a", "b", "NEW", "c", "d"];
        let result = compute_diff(&a, &b);
        assert_eq!((result.added, result.removed, result.modified), (1, 0, 0));
    }

    #[test]
    fn test_compute_diff_deletion_in_the_middle() {
        let a = vec!["a", "b", "GONE", "c", "d"];
        let b = vec!["a", "b", "c", "d"];
        let result = compute_diff(&a, &b);
        assert_eq!((result.added, result.removed, result.modified), (0, 1, 0));
        match &result.changes()[0] {
            DiffChange::Removed { line1, text } => {
                assert_eq!(*text, "GONE");
                assert_eq!(*line1, 3, "line number is the old file's");
            }
            other => panic!("expected Removed, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_diff_reports_line_numbers_after_a_shift() {
        // A change *after* an insertion must still name its own line, not an
        // offset one.
        let a = vec!["a", "b", "let x = 1;"];
        let b = vec!["NEW", "a", "b", "let x = 2;"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 1);
        assert_eq!(result.modified, 1);
        assert!(result
            .changes()
            .iter()
            .any(|c| matches!(c, DiffChange::Modified { line1: 3, old, new }
                if *old == "let x = 1;" && *new == "let x = 2;")));
    }

    #[test]
    fn test_compute_diff_adjacent_replacement_pairs_by_similarity() {
        // Two lines replaced in place. First pair is similar -> Modified;
        // second is not -> Removed + Added.
        let a = vec!["let x = 1;", "aaaa"];
        let b = vec!["let x = 2;", "zzzz"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 1);
        assert_eq!(result.added, 1);
        assert_eq!(result.removed, 1);
        assert!(result.unaligned.is_none());
    }

    #[test]
    fn test_compute_diff_does_not_pair_across_matched_lines() {
        // A deletion at line 1 and an insertion at line 41 are adjacent in the
        // ops vector, because matched lines used to emit nothing. `Op::Keep`
        // keeps the 40 shared lines between them, so they stay unpaired
        // instead of being reported as one modification with counts zeroed.
        let shared: Vec<String> = (0..40).map(|i| format!("shared {}", i)).collect();
        let mut a = vec!["delete_me_x"];
        a.extend(shared.iter().map(|s| s.as_str()));
        let mut b: Vec<&str> = shared.iter().map(|s| s.as_str()).collect();
        b.push("delete_me_y");

        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 0, "unrelated lines must not pair");
        assert_eq!(result.removed, 1);
        assert_eq!(result.added, 1);
    }

    #[test]
    fn test_compute_diff_reorder_is_not_a_line_modified_into_itself() {
        // Sorting an import block: `use zeta::A;` moves, it is not rewritten.
        let a = vec!["use zeta::A;", "use beta::B;", "use gamma::C;"];
        let b = vec!["use beta::B;", "use gamma::C;", "use zeta::A;"];
        let result = compute_diff(&a, &b);

        assert_eq!(result.modified, 0);
        assert_eq!(result.removed, 1);
        assert_eq!(result.added, 1);
        for change in &result.changes() {
            if let DiffChange::Modified { old, new, .. } = change {
                panic!("reported {:?} as modified into {:?}", old, new);
            }
        }
    }

    #[test]
    fn test_compute_diff_added_line_numbers_come_from_file2() {
        // The `Added` half of an unpaired replacement used file1's line
        // number, silently mixing two numbering conventions in one output.
        let a = vec!["x", "AAAA"];
        let b = vec!["NEW", "x", "BBBB"];
        let result = compute_diff(&a, &b);

        let added: Vec<usize> = result
            .changes()
            .iter()
            .filter_map(|c| match c {
                DiffChange::Added { line2, text } if *text == "BBBB" => Some(*line2),
                _ => None,
            })
            .collect();
        assert_eq!(added, vec![3], "BBBB is line 3 of file2");
    }

    #[test]
    fn test_compute_diff_equal_lengths_have_no_cliff() {
        // In-place rewrites of a 10,000-line file: past the trace budget
        // the positional fallback still names every changed line, so one more
        // rewrite cannot take the output from complete to nothing.
        let render = |rewrites: usize| {
            let a_lines: Vec<String> = (0..10_000).map(|i| format!("line {}", i)).collect();
            let b_lines: Vec<String> = (0..10_000)
                .map(|i| {
                    if i < rewrites {
                        format!("line {} REWRITTEN", i)
                    } else {
                        format!("line {}", i)
                    }
                })
                .collect();
            let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
            let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();
            compute_diff(&a, &b)
        };

        // A rewritten pair renders as one `Modified` when the two texts are
        // similar enough and as `Removed` + `Added` otherwise, so the count
        // that matters is how many changed lines are named at all.
        let named = |d: &DiffResult| d.modified + d.removed;

        // Sweep across the budget rather than pinning where it sits: the trace
        // budget tracks the shape of the change, so a run of contiguous
        // rewrites gets a narrower diagonal window than scattered ones and
        // aligns further. What must hold at every density is that a changed
        // line is named — by the aligner or by the positional fallback.
        let mut aligned = 0;
        let mut positional = 0;
        for rewrites in [349, 500, 501, 700, 1_000, 4_999] {
            let result = render(rewrites);
            assert!(
                result.unaligned.is_none(),
                "{} rewrites must produce a change list",
                rewrites
            );
            assert_eq!(
                named(&result),
                rewrites,
                "{} rewrites must all be named",
                rewrites
            );
            if result.positional {
                positional += 1;
            } else {
                aligned += 1;
            }
        }
        assert!(aligned > 0 && positional > 0, "both branches must be covered");
    }

    #[test]
    fn test_compute_diff_positional_fallback_is_bounded() {
        // Two wholly different equal-length files: listing every line would
        // build a change list the size of both files. The count is exact
        // because equal lengths make it a single pass.
        let a_lines: Vec<String> = (0..20_000).map(|i| format!("alpha {}", i)).collect();
        let b_lines: Vec<String> = (0..20_000).map(|i| format!("bravo {}", i)).collect();
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert_eq!(result.unaligned, Some(Unaligned::DifferingLines(20_000)));
        assert!(result.changes().is_empty());
        assert!(!result.positional);

        let (out, code) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            &a_lines.join("\n"),
            &b_lines.join("\n"),
        );
        assert_eq!(code, 1);
        assert!(out.contains("20000 lines differ"), "got:\n{}", out);
        assert!(
            !out.contains("differences fall between"),
            "count is exact, so no region bounds, got:\n{}",
            out
        );
    }

    #[test]
    fn test_render_positional_fallback_says_so() {
        // Covers `render_file_diff`, not what `run` prints: at near-total
        // rewrite the render exceeds raw and `never_worse` substitutes the two
        // files, taking the label with it. That threshold is far above the
        // densities this branch exists for (10,000 lines / 4,999 rewritten
        // still renders), and there the raw concatenation is the better answer.
        let content1: String = (0..2400).map(|i| format!("line {}\n", i)).collect();
        let content2: String = (0..2400)
            .map(|i| format!("line {} REWRITTEN\n", i))
            .collect();
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert_eq!(code, 1);
        assert!(out.contains("paired by line position"), "got:\n{}", out);
        for line in [1usize, 1200, 2400] {
            assert!(
                out.contains(&format!("line {} REWRITTEN", line - 1)),
                "line {} must be named, got:\n{}",
                line,
                out
            );
        }
    }

    #[test]
    fn test_render_over_cap_message_counts_lines_not_operations() {
        // Unequal lengths, so no positional fallback. The first clause must not
        // say "over 1410 lines" when 705 is the floor the cap actually implies:
        // the aligner gave up at round 1410, and an in-place rewrite is two
        // rounds, so half of it is all that is proven.
        let content1: String = (0..2000).map(|i| format!("a{}\n", i)).collect();
        let content2: String = (0..2001).map(|i| format!("b{}\n", i)).collect();
        let (out, _) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert!(out.contains("at least 705 lines differ"), "got:\n{}", out);
        // The region is stated as line bounds in each file. A figure shaped
        // like a count would read as the amount of change, which it is not:
        // scattered edits in a large file span nearly the whole file.
        assert!(
            out.contains("differences fall between lines 1-2000 of a.txt and 1-2001 of b.txt"),
            "region must be stated as line bounds, got:\n{}",
            out
        );
        assert!(!out.contains("spans"), "no span-shaped figure, got:\n{}", out);
    }

    #[test]
    fn test_render_region_bounds_are_lines_not_a_change_count() {
        // 1,101 changed lines in a 10,000-line file, first at 5 and last at
        // 9,500, so the changed region is ~9,495 lines. Stating that as a figure
        // next to "at least 705 lines differ" would read as 9x the real change.
        let a_lines: Vec<String> = (0..10000).map(|i| format!("line {}", i)).collect();
        let mut b_lines = a_lines.clone();
        for i in 0..1100 {
            b_lines[4 + i * 8] = format!("line {} EDITED", 4 + i * 8);
        }
        b_lines.insert(9500, "INSERTED".to_string());
        let content1: String = a_lines
            .iter()
            .map(|l| format!("{}\n", l))
            .collect::<String>();
        let content2: String = b_lines
            .iter()
            .map(|l| format!("{}\n", l))
            .collect::<String>();

        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);
        assert_eq!(code, 1);
        assert!(out.contains("at least 705 lines differ"), "got:\n{}", out);
        assert!(
            out.contains("differences fall between lines 5-"),
            "bounds start at the first difference, got:\n{}",
            out
        );
    }

    #[test]
    fn test_compute_diff_pure_insertion_past_cap_still_lists_every_line() {
        // 701 appended lines, which under an edit-distance cap was one past it
        // and exhausted the aligner's rounds. One side of the trimmed middle is empty, so the
        // script is a single insertion run and needs no search at all.
        let a_lines: Vec<String> = (0..50).map(|i| format!("keep {}", i)).collect();
        let mut b_lines = a_lines.clone();
        for i in 0..701 {
            b_lines.push(format!("appended {}", i));
        }
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert!(
            result.unaligned.is_none(),
            "a pure insertion is never too different to align, got: {:?}",
            result.unaligned
        );
        assert_eq!(result.added, 701);
        assert_eq!(result.removed, 0);
        assert_eq!(result.modified, 0);
        assert_eq!(result.changes().len(), 701);
        match &result.changes()[0] {
            DiffChange::Added { line2, text } => {
                assert_eq!((*line2, *text), (51, "appended 0"));
            }
            other => panic!("expected an addition at line 51, got {:?}", other),
        }
    }

    #[test]
    fn test_compute_diff_empty_against_populated_lists_every_line() {
        let b_lines: Vec<String> = (0..1050).map(|i| format!("line {}", i)).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&[], &b);
        assert!(result.unaligned.is_none());
        assert_eq!(result.added, 1050);
        assert_eq!(result.changes().len(), 1050);

        let reverse = compute_diff(&b, &[]);
        assert!(reverse.unaligned.is_none());
        assert_eq!(reverse.removed, 1050);
        assert_eq!(reverse.changes().len(), 1050);
    }

    /// Deterministic pseudo-random source, so a failure reproduces exactly.
    fn lcg(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state >> 33
    }

    #[test]
    fn test_myers_ops_script_reconstructs_the_second_file() {
        // The aligner explores a banded diagonal window, so the trace no longer
        // covers `[-d, d]` and the backtrack reads it through a per-round base.
        // An off-by-one there produces a plausible-looking script that does not
        // reconstruct file2, which no single hand-written case would catch.
        let mut seed = 0x5eed_1234_u64;
        let mut aligned = 0usize;

        for _ in 0..4_000 {
            let n = (lcg(&mut seed) % 40) as usize;
            let m = (lcg(&mut seed) % 40) as usize;
            let alphabet = 1 + (lcg(&mut seed) % 4);
            let a_lines: Vec<String> = (0..n)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let b_lines: Vec<String> = (0..m)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
            let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

            let ops = match myers_ops(&a, &b) {
                Ok(Aligned::Script(ops)) => ops,
                Ok(Aligned::TooManySteps { .. }) | Err(_) => continue,
            };
            aligned += 1;

            // Replay the script: `Keep(run)` consumes `run` lines from each
            // side, `Del` one from file1, `Ins` one from file2.
            let (mut i, mut j) = (0usize, 0usize);
            let mut rebuilt: Vec<&str> = Vec::new();
            for op in &ops {
                match op {
                    Op::Keep(run) => {
                        assert!(*run > 0, "an empty Keep separates nothing");
                        for _ in 0..*run {
                            assert_eq!(a.get(i), b.get(j), "Keep must pair equal lines");
                            rebuilt.push(a[i]);
                            i += 1;
                            j += 1;
                        }
                    }
                    Op::Del(text) => {
                        assert_eq!(a[i], text.as_str(), "Del names file1's text");
                        i += 1;
                    }
                    Op::Ins(text) => {
                        assert_eq!(b[j], text.as_str(), "Ins names file2's text");
                        rebuilt.push(b[j]);
                        j += 1;
                    }
                }
            }
            assert_eq!((i, j), (n, m), "the script must consume both files");
            assert_eq!(rebuilt, b, "the script must reconstruct file2");
        }

        assert!(aligned > 3_000, "most pairs must align, got {}", aligned);
    }

    #[test]
    fn test_myers_ops_script_is_minimal() {
        // Minimality against a brute-force LCS: the banded window must not drop
        // a diagonal an optimal path needs.
        fn lcs_len(a: &[&str], b: &[&str]) -> usize {
            let mut prev = vec![0usize; b.len() + 1];
            for x in a {
                let mut cur = vec![0usize; b.len() + 1];
                for (j, y) in b.iter().enumerate() {
                    cur[j + 1] = if x == y {
                        prev[j] + 1
                    } else {
                        cur[j].max(prev[j + 1])
                    };
                }
                prev = cur;
            }
            prev[b.len()]
        }

        let mut seed = 0xfeed_4321_u64;
        for _ in 0..2_000 {
            let n = (lcg(&mut seed) % 25) as usize;
            let m = (lcg(&mut seed) % 25) as usize;
            let alphabet = 1 + (lcg(&mut seed) % 3);
            let a_lines: Vec<String> = (0..n)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let b_lines: Vec<String> = (0..m)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
            let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

            let Ok(Aligned::Script(ops)) = myers_ops(&a, &b) else {
                continue;
            };
            let edits = ops.iter().filter(|op| !matches!(op, Op::Keep(_))).count();
            assert_eq!(
                edits,
                n + m - 2 * lcs_len(&a, &b),
                "script must be minimal for {:?} vs {:?}",
                a,
                b
            );
        }
    }

    /// Apply a classic diff to `a` and return what it produces, checking that
    /// every hunk's `<` bodies name file1 at the lines the header claims and
    /// every `>` body names file2 at its own.
    fn replay_classic(script: &str, a: &[&str], b: &[&str]) -> Vec<String> {
        fn range(spec: &str) -> (usize, usize) {
            match spec.split_once(',') {
                Some((s, e)) => (s.parse().unwrap(), e.parse().unwrap()),
                None => {
                    let n = spec.parse().unwrap();
                    (n, n)
                }
            }
        }

        let mut out: Vec<String> = Vec::new();
        let mut cursor = 0usize; // file1 lines already emitted or dropped
        let mut lines = script.lines().peekable();

        while let Some(header) = lines.next() {
            let op = header
                .chars()
                .find(|c| matches!(c, 'a' | 'c' | 'd'))
                .expect("hunk header carries an operation");
            let (left, right) = header.split_once(op).unwrap();

            let mut old_body = Vec::new();
            let mut new_body = Vec::new();
            while let Some(line) = lines.peek() {
                if let Some(rest) = line.strip_prefix("< ") {
                    old_body.push(rest.to_string());
                } else if let Some(rest) = line.strip_prefix("> ") {
                    new_body.push(rest.to_string());
                } else if *line != "---" {
                    break;
                }
                lines.next();
            }

            let (start1, end1, start2, end2) = match op {
                'a' => {
                    let anchor: usize = left.parse().unwrap();
                    let (s2, e2) = range(right);
                    (anchor + 1, anchor, s2, e2)
                }
                'd' => {
                    let (s1, e1) = range(left);
                    (s1, e1, 0, 0)
                }
                _ => {
                    let (s1, e1) = range(left);
                    let (s2, e2) = range(right);
                    (s1, e1, s2, e2)
                }
            };

            // Copy the untouched file1 lines that precede this hunk.
            assert!(start1 > cursor, "hunks must not overlap: {header}");
            for line in a.iter().take(start1 - 1).skip(cursor) {
                out.push((*line).to_string());
            }
            cursor = start1 - 1;

            if op != 'a' {
                assert_eq!(
                    old_body,
                    a[start1 - 1..end1]
                        .iter()
                        .map(|l| (*l).to_string())
                        .collect::<Vec<_>>(),
                    "`<` body must name file1 at {header}"
                );
                cursor = end1;
            }
            if op != 'd' {
                assert_eq!(
                    new_body,
                    b[start2 - 1..end2]
                        .iter()
                        .map(|l| (*l).to_string())
                        .collect::<Vec<_>>(),
                    "`>` body must name file2 at {header}"
                );
                out.extend(new_body);
            }
        }

        out.extend(a.iter().skip(cursor).map(|l| (*l).to_string()));
        out
    }

    #[test]
    fn test_classic_diff_replays_into_the_second_file() {
        // The classic renderer numbers each hunk in both files, and after an
        // insertion the two frames disagree. Deriving one from the other — or
        // pairing a replacement by equal line numbers — produces a script that
        // still parses and still looks like a diff, so only replaying it
        // catches the mislabelling.
        let mut seed = 0x0c1a_551c_u64;
        let mut replayed = 0usize;

        for _ in 0..3_000 {
            let n = (lcg(&mut seed) % 25) as usize;
            let m = (lcg(&mut seed) % 25) as usize;
            let alphabet = 1 + (lcg(&mut seed) % 5);
            let a_lines: Vec<String> = (0..n)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let b_lines: Vec<String> = (0..m)
                .map(|_| format!("L{}", lcg(&mut seed) % alphabet))
                .collect();
            let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
            let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

            let diff = compute_diff(&a, &b);
            if diff.unaligned.is_some() {
                continue;
            }
            replayed += 1;

            let script = format_classic_diff(&diff);
            assert_eq!(
                replay_classic(&script, &a, &b),
                b_lines,
                "script must rebuild file2 for {:?} vs {:?}:\n{}",
                a,
                b,
                script
            );
        }

        assert!(replayed > 2_500, "most pairs must render, got {}", replayed);
    }

    #[test]
    fn test_compute_diff_lopsided_pair_aligns_past_the_edit_distance() {
        // One line against five thousand, sharing a line in the middle so the
        // prefix/suffix trim cannot empty either side. The minimal script is
        // 4,999 insertions, so an edit-distance cap refused it — but only one
        // deletion is possible, so the diagonal window stays three wide and the
        // whole trace is a few thousand cells.
        let b_lines: Vec<String> = (0..5_000)
            .map(|i| {
                if i == 2_500 {
                    "KEEP".to_string()
                } else {
                    format!("ins {}", i)
                }
            })
            .collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&["KEEP"], &b);
        assert!(
            result.unaligned.is_none(),
            "a lopsided pair must still align, got: {:?}",
            result.unaligned
        );
        assert!(!result.positional);
        assert_eq!(result.added, 4_999);
        assert_eq!(result.removed, 0);
        assert_eq!(result.modified, 0);
    }

    #[test]
    fn test_compute_diff_listing_is_bounded_by_bytes_not_only_lines() {
        // 3,000 changes is inside `POSITIONAL_CHANGE_CAP`, but each holds two
        // 1,000-byte strings, so the list would clone six megabytes before
        // `never_worse` could discard it. The count stays exact.
        let long_a = "a".repeat(1_000);
        let long_b = "b".repeat(1_000);
        let a_lines: Vec<String> = (0..3_000).map(|_| long_a.clone()).collect();
        let b_lines: Vec<String> = (0..3_000).map(|_| long_b.clone()).collect();
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert_eq!(result.unaligned, Some(Unaligned::DifferingLines(3_000)));
        assert!(result.changes().is_empty());

        // Short lines at the same count still list.
        let short_a: Vec<String> = (0..3_000).map(|i| format!("a{}", i)).collect();
        let short_b: Vec<String> = (0..3_000).map(|i| format!("b{}", i)).collect();
        let sa: Vec<&str> = short_a.iter().map(|s| s.as_str()).collect();
        let sb: Vec<&str> = short_b.iter().map(|s| s.as_str()).collect();
        let short = compute_diff(&sa, &sb);
        assert!(short.unaligned.is_none(), "3,000 short changes still list");
    }

    #[test]
    fn test_compute_diff_one_sided_run_is_bounded() {
        // The listing is exact but not unbounded: past `POSITIONAL_CHANGE_CAP`
        // the count says more than ten thousand lines of change list would.
        let b_lines: Vec<String> = (0..POSITIONAL_CHANGE_CAP + 1)
            .map(|i| format!("line {}", i))
            .collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&[], &b);
        assert_eq!(
            result.unaligned,
            Some(Unaligned::DifferingLines(POSITIONAL_CHANGE_CAP + 1))
        );
        assert!(result.changes().is_empty());
    }

    #[test]
    fn test_render_reference_frames_are_labelled() {
        // `-` and `+` numbers come from different files; the legend appears
        // only when the output actually mixes them.
        let (mixed, _) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "x\nAAAAAAAA\n",
            "NEW\nx\nZZZZZZZZ\n",
        );
        assert!(
            mixed.contains("(- = file 1; + = file 2)"),
            "got:\n{}",
            mixed
        );

        let (modified_only, _) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "a: 1\n", "a: 2\n");
        assert!(
            !modified_only.contains("; + = "),
            "no legend when nothing mixes frames, got:\n{}",
            modified_only
        );
    }

    #[test]
    fn test_render_frame_legend_covers_added_beside_modified() {
        // `+` is numbered in file2 and `~` in file1, so the two mix frames just
        // as `-` and `+` do. Gating the legend on a `-` being present left this
        // shape bare: `value = alpha2` is at line 5 of b.txt, not the 4 the `~`
        // shows, and nothing said which file the number belonged to.
        let (out, _) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "a\nb\nc\nvalue = alpha\n",
            "a\nEXTRA\nb\nc\nvalue = alpha2\n",
        );

        assert!(
            out.contains("(~ = file 1; + = file 2)"),
            "an insertion above a modification mixes frames, got:\n{}",
            out
        );
    }

    #[test]
    fn test_render_frame_legend_names_only_the_markers_present() {
        // The legend exists to stop a line-number misread, so one that
        // describes different output than the reader is looking at is worse
        // than none: with no `-` on screen it announced a `-` frame with no
        // lines in it and said nothing about the `~` lines that were there.
        let (added_and_modified, _) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "a\nb\nc\nvalue = alpha\n",
            "a\nEXTRA\nb\nc\nvalue = alpha2\n",
        );
        assert!(
            !added_and_modified.lines().any(|l| l.starts_with('-')),
            "no `-` lines in this shape, got:\n{}",
            added_and_modified
        );
        assert!(
            !added_and_modified.contains("(- "),
            "the legend must not name an absent frame, got:\n{}",
            added_and_modified
        );

        // A `-` with no `~`: the legend names the `-` alone.
        let (added_and_removed, _) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "a\nb\nzzzz\n",
            "a\nEXTRA\nb\nqqqq\n",
        );
        assert!(
            added_and_removed.contains("(- = file 1; + = file 2)"),
            "got:\n{}",
            added_and_removed
        );

        // Both frames on screen: the legend names both.
        let (all_three, _) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "a\nb\nzzzz\nvalue = alpha\n",
            "a\nEXTRA\nb\nqqqq\nvalue = alpha2\n",
        );
        assert!(
            all_three.contains("(-,~ = file 1; + = file 2)"),
            "got:\n{}",
            all_three
        );
    }

    #[test]
    fn test_render_added_only_needs_no_frame_legend() {
        // Every listed line comes from file2. One frame, no note.
        let (out, _) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "a\nb\n", "a\nNEW\nb\n");

        assert!(out.contains("+   2 NEW"), "got:\n{}", out);
        assert!(
            !out.contains("; + = "),
            "one frame needs no legend, got:\n{}",
            out
        );
    }

    #[test]
    fn test_compute_diff_aligned_script_past_the_count_cap_refuses() {
        // The listing path the band made reachable. One line against 60,000
        // sharing a line in the middle aligns cheaply — the window stays three
        // diagonals wide — and would then build 59,999 changes, 11x
        // `POSITIONAL_CHANGE_CAP`, from an input the empty-file case refuses at
        // the same size for free.
        let a = vec!["SHARED"];
        let b_lines: Vec<String> = (0..60000)
            .map(|i| {
                if i == 30000 {
                    "SHARED".to_string()
                } else {
                    format!("x{}", i)
                }
            })
            .collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert_eq!(
            result.unaligned,
            Some(Unaligned::EditScript {
                removed: 0,
                added: 59999
            })
        );
        assert!(result.changes().is_empty());
    }

    #[test]
    fn test_aligned_script_over_the_count_cap_is_never_materialised() {
        // The count budget is spent before the script exists. `myers_ops` knows
        // the edit distance the moment it reaches the end, so the cheap-to-
        // answer shape — one line against 60,000, which the band aligns in a
        // three-diagonal window — refuses there rather than cloning 59,999
        // lines into an `Op` vector it is about to throw away.
        let a = vec!["SHARED"];
        let b_lines: Vec<String> = (0..60000)
            .map(|i| {
                if i == 30000 {
                    "SHARED".to_string()
                } else {
                    format!("x{}", i)
                }
            })
            .collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        match myers_ops(&a, &b) {
            Ok(Aligned::TooManySteps { removed, added }) => {
                assert_eq!((removed, added), (0, 59999));
            }
            Ok(Aligned::Script(ops)) => panic!("built a {}-step script it cannot list", ops.len()),
            Err(d) => panic!("the band must reach this pair, gave up at d = {}", d),
        }
    }

    #[test]
    fn test_listing_cap_counts_positions_not_rendered_lines() {
        // `POSITIONAL_CHANGE_CAP` bounds differing positions, and a position
        // whose two texts share no characters renders as a `-` and a `+`. The
        // constant's own comment has to say that: at exactly the cap the output
        // is twice its value, which reads as a broken bound if the cap is
        // documented as a line count.
        // Disjoint character sets, so `similarity` can never rate a pair as
        // `Modified` and fold two rendered lines back into one.
        let encode = |i: usize, alphabet: &[u8]| -> String {
            i.to_string()
                .bytes()
                .map(|d| alphabet[(d - b'0') as usize] as char)
                .collect()
        };
        let content1: String = (0..POSITIONAL_CHANGE_CAP)
            .map(|i| format!("{}\n", encode(i, b"abcdefghij")))
            .collect();
        let content2: String = (0..POSITIONAL_CHANGE_CAP)
            .map(|i| format!("{}\n", encode(i, b"PQRSTUVWXY")))
            .collect();

        let diff = changes_of(&content1, &content2);
        assert!(diff.unaligned.is_none(), "exactly at the cap is admitted");
        assert_eq!(
            (diff.added, diff.removed, diff.modified),
            (POSITIONAL_CHANGE_CAP, POSITIONAL_CHANGE_CAP, 0)
        );
        assert_eq!(
            format_diff_changes(&diff).lines().count(),
            2 * POSITIONAL_CHANGE_CAP,
            "each differing position renders two lines"
        );
    }

    #[test]
    fn test_compute_diff_aligned_script_past_the_byte_cap_refuses() {
        // Two changes, past the byte budget. A count cap alone would pass this
        // and clone 2.2MB twice on the way to the render.
        let big1 = "x".repeat(1_100_000);
        let big2 = "y".repeat(1_100_000);
        let a = vec!["head", big1.as_str(), "tail"];
        let b = vec!["head", big2.as_str(), "tail"];

        let result = compute_diff(&a, &b);
        assert_eq!(
            result.unaligned,
            Some(Unaligned::EditScript {
                removed: 1,
                added: 1
            })
        );
        assert!(result.changes().is_empty());
    }

    #[test]
    fn test_render_edit_script_over_cap_states_both_sides() {
        // Both counts, because the script knows both exactly and one figure
        // would not: the runs have not been paired, and a pairing turns two
        // steps into either one `~` or one `-` plus one `+`.
        let content1 = "SHARED\n".to_string();
        let content2: String = (0..60000)
            .map(|i| {
                if i == 30000 {
                    "SHARED\n".to_string()
                } else {
                    format!("x{}\n", i)
                }
            })
            .collect();
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert_eq!(code, 1);
        assert!(
            out.contains("only in a.txt: 0 lines, only in b.txt: 59999 lines"),
            "got:\n{}",
            out
        );
        assert!(out.contains("rtk proxy diff"), "got:\n{}", out);
        assert!(
            out.lines().count() <= 2,
            "the refusal must not grow with the input, got:\n{}",
            out
        );
    }

    #[test]
    fn test_compute_diff_moderately_changed_file_still_aligns() {
        // An 18%-changed 2,000-line file. One inserted line makes the lengths
        // unequal, which removes the positional fallback, so an aligner that
        // gives up here lists nothing at all. The trace budget has to reach
        // further than the listing budget refuses at, or the refusal is a cliff.
        let a_lines: Vec<String> = (0..2000).map(|i| format!("line {}", i)).collect();
        let mut b_lines = a_lines.clone();
        for line in b_lines.iter_mut().take(360) {
            *line = format!("{} EDITED", line);
        }
        b_lines.insert(1500, "INSERTED".to_string());
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert!(result.unaligned.is_none(), "must still align");
        assert_eq!(result.modified, 360);
        assert_eq!(result.added, 1);
        assert_eq!(result.removed, 0);
    }

    #[test]
    fn test_compute_diff_scattered_rewrites_align_to_about_seven_hundred() {
        // What `MAX_TRACE_CELLS` is really choosing, pinned so a change to the
        // constant has to restate it. Scattered in-place rewrites in a file far
        // longer than the change get the full diagonal window, which is the
        // worst case for the trace.
        let base: Vec<String> = (0..10000).map(|i| format!("line {}", i)).collect();
        let rewrite = |count: usize| {
            let mut b = base.clone();
            for i in 0..count {
                b[i * 9] = format!("line {} EDITED", i * 9);
            }
            b
        };

        let a: Vec<&str> = base.iter().map(|s| s.as_str()).collect();

        let under = rewrite(700);
        let under_ref: Vec<&str> = under.iter().map(|s| s.as_str()).collect();
        let aligned = compute_diff(&a, &under_ref);
        assert!(!aligned.positional, "700 rewrites must still align");
        assert_eq!(aligned.modified, 700);

        let over = rewrite(720);
        let over_ref: Vec<&str> = over.iter().map(|s| s.as_str()).collect();
        let fell_back = compute_diff(&a, &over_ref);
        assert!(
            fell_back.positional,
            "past the budget equal lengths pair by position rather than listing nothing"
        );
    }

    #[test]
    fn test_render_positional_fallback_has_no_frame_legend() {
        // `positional_changes` numbers both halves of a dissimilar pair from
        // the same position, so there is one frame. The legend would tell the
        // reader to read two identical numbers as belonging to different files.
        let content1: String = (0..2400).map(|i| format!("aaa{}\n", i)).collect();
        let content2: String = (0..2400).map(|i| format!("zzz{}\n", i)).collect();
        let (out, _) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert!(out.contains("paired by line position"), "got:\n{}", out);
        assert!(out.contains("-   1 aaa0"), "got:\n{}", out);
        assert!(out.contains("+   1 zzz0"), "got:\n{}", out);
        assert!(
            !out.contains("; + = "),
            "one frame needs no legend, got:\n{}",
            out
        );
    }

    #[test]
    fn test_compute_diff_two_far_apart_edits_still_align() {
        // The cap is on the amount of change, not on the span between the
        // first and last edit. Two 2100-line files differing at lines 5 and
        // 2095 have a 2091-line changed region and an edit distance of 4.
        let a_lines: Vec<String> = (0..2100).map(|i| format!("line {}", i)).collect();
        let mut b_lines = a_lines.clone();
        b_lines[4] = "line 4 EDITED".to_string();
        b_lines[2094] = "line 2094 EDITED".to_string();
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert!(
            result.unaligned.is_none(),
            "two edits must not exhaust the aligner"
        );
        assert_eq!(result.modified, 2, "both edits are in-place rewrites");
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);

        let lines: Vec<usize> = result
            .changes()
            .iter()
            .filter_map(|c| match c {
                DiffChange::Modified { line1, .. } => Some(*line1),
                _ => None,
            })
            .collect();
        assert_eq!(lines, vec![5, 2095], "line numbers, not region offsets");
    }

    #[test]
    fn test_compute_diff_past_edit_distance_cap_reports_a_region_not_counts() {
        // Wholly different middles of unequal length, so neither the aligner
        // nor the positional fallback applies. The counts stay zero rather
        // than restating the region span as if it had been measured.
        let a_lines: Vec<String> = (0..2001).map(|i| format!("a{}", i)).collect();
        let b_lines: Vec<String> = (0..2002).map(|i| format!("b{}", i)).collect();
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        let result = compute_diff(&a, &b);
        assert_eq!(
            result.unaligned,
            Some(Unaligned::RegionBounds {
                differing_floor: 705,
                first: 1,
                last1: 2001,
                last2: 2002
            })
        );
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert_eq!(result.modified, 0);
        assert!(result.changes().is_empty());
    }

    #[test]
    fn test_render_far_apart_edits_reports_two_changes_not_a_region() {
        // The shape the positional compare on develop got right, and that a
        // size-keyed cap would have turned into a confident "+2091 / -2091".
        let content1: String = (0..2100).map(|i| format!("line {}\n", i)).collect();
        let content2: String = (0..2100)
            .map(|i| match i {
                4 => "line 4 EDITED\n".to_string(),
                2094 => "line 2094 EDITED\n".to_string(),
                _ => format!("line {}\n", i),
            })
            .collect();
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert_eq!(code, 1);
        assert!(out.contains("~   5 line 4 → line 4 EDITED"), "got:\n{}", out);
        assert!(out.contains("~2095 line 2094 → line 2094 EDITED"), "got:\n{}", out);
        assert_eq!(out.lines().count(), 2, "two changes, nothing else, got:\n{}", out);
        assert!(!out.contains("2091"), "region size must not appear, got:\n{}", out);
    }

    #[test]
    fn test_render_past_edit_distance_cap_names_the_region_as_a_region() {
        let content1: String = (0..2001).map(|i| format!("a{}\n", i)).collect();
        let content2: String = (0..2002).map(|i| format!("b{}\n", i)).collect();
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &content2);

        assert_eq!(code, 1);
        assert!(
            out.contains("differences fall between lines 1-2001 of a.txt and 1-2002 of b.txt"),
            "got:\n{}",
            out
        );
        assert!(out.contains("too different to align"), "got:\n{}", out);
        assert!(!out.contains("added"), "no count is claimed, got:\n{}", out);
        assert!(!out.contains("text matches"), "got:\n{}", out);
    }

    #[test]
    fn test_crlf_line_numbers_ignores_an_unterminated_tail() {
        // A file ending in a bare `\r` has no newline there, so that `\r` is
        // content rather than half a CRLF terminator.
        assert_eq!(crlf_line_numbers("x\r\ny\r"), vec![1]);
        assert_eq!(crlf_line_numbers("x\ny\r"), Vec::<usize>::new());
        assert_eq!(crlf_line_numbers("a\r\nb\r\n"), vec![1, 2]);

        let (out, _) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "x\r\ny\r", "x\ny\r");
        assert!(out.contains("1 CRLF vs 0 CRLF"), "got:\n{}", out);
    }

    /// What `run` prints, minus the file I/O and the tracking.
    fn shown_for(file1: &str, file2: &str, content1: &str, content2: &str) -> (String, i32) {
        let both_files = format!("{}\n---\n{}", content1, content2);
        let comparison = compare_files(content1, content2);
        let fallback = classic_fallback(&comparison);
        let (rendered, code) = render_diff(Path::new(file1), Path::new(file2), &comparison);
        let shown = select_file_diff_output(&comparison, &fallback, &both_files, &rendered);
        (shown.to_string(), code)
    }

    #[test]
    fn test_invisible_difference_message_survives_on_a_one_line_pair() {
        // The message has a floor of its own — its shortest form is ~20 tokens —
        // and a fixed allowance above raw sat under it, so a one-line pair lost
        // the message 90% of the time and printed two indistinguishable blobs
        // instead. The message is shown whenever the case arises: it competes
        // with no change list, and the raw fallback answers worse at any size.
        for (content1, content2) in [
            ("alpha\nbeta", "alpha\r\nbeta\r\n"),
            ("a\n", "a"),
            ("a\r\nb\n", "a\nb\r\n"),
            ("x\n", "x\r\n"),
        ] {
            let (shown, code) = shown_for("n1", "n2", content1, content2);
            assert_eq!(code, 1);
            assert!(
                shown.contains("whitespace or line endings"),
                "{:?} vs {:?} must explain the difference, got:\n{}",
                content1,
                content2,
                shown
            );
            assert!(
                !shown.contains("\n---\n"),
                "{:?} vs {:?} must not print the two blobs, got:\n{}",
                content1,
                content2,
                shown
            );
        }
    }

    #[test]
    fn test_invisible_difference_message_is_independent_of_path_length() {
        // The caller typed both paths, so their length says nothing about
        // whether the diagnostic is worth showing.
        let content1 = "a,b\nc,d\ne,f\n";
        let content2 = "a,b\r\nc,d\r\ne,f\r\n";
        let (shown, _) = shown_for(
            "/home/user/projects/rtk/tests/fixtures/expected_output.csv",
            "/home/user/projects/rtk/tests/fixtures/actual_output.csv",
            content1,
            content2,
        );
        assert!(shown.contains("0 CRLF vs 3 CRLF"), "got:\n{}", shown);
    }

    #[test]
    fn test_describe_invisible_difference_never_prints_equal_byte_counts() {
        // Same CRLF count on both sides, different placement. The old fallback
        // printed "5 vs 5 bytes", which reads as "no difference at all".
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "a\r\nb\n", "a\nb\r\n");

        assert_eq!(code, 1);
        assert!(!out.contains("5 vs 5 bytes"), "got:\n{}", out);
        assert!(
            out.contains("1 CRLF on each side, first differing at line 1"),
            "got:\n{}",
            out
        );
    }

    // --- classic diff fallback, baseline and guard routing ---

    #[test]
    fn test_never_worse_fallback_is_a_classic_diff() {
        // Three appended lines: classic prints one header and a two-byte
        // prefix per line, the condensed render a six-byte one, so the guard
        // hands back the classic diff rather than the two files.
        let comparison = compare_files("keep\n", "keep\nnew 1\nnew 2\nnew 3\n");
        let fallback = classic_fallback(&comparison);
        let (rendered, code) = render_diff(Path::new("before"), Path::new("after"), &comparison);
        let shown = select_file_diff_output(&comparison, &fallback, "", &rendered);

        assert_eq!(code, 1);
        assert!(rendered.len() > fallback.len(), "fixture must make classic the smaller output");
        assert_eq!(shown, "1a2,4\n> new 1\n> new 2\n> new 3\n");
    }

    #[test]
    fn test_tracking_baseline_never_books_a_loss() {
        // Two unrelated files: the classic diff carries both of them plus the
        // "< " / "> " markers, so it is bigger than a plain dump. Measuring
        // against the dump used to record negative savings.
        let content1: String = (0..40).map(|i| format!("old line {i}\n")).collect();
        let content2: String = (0..40).map(|i| format!("brand new content {i}\n")).collect();
        let both_files = format!("{}\n---\n{}", content1, content2);

        let comparison = compare_files(&content1, &content2);
        let fallback = classic_fallback(&comparison);
        let (rendered, _) = render_diff(Path::new("a"), Path::new("b"), &comparison);
        let shown = select_file_diff_output(&comparison, &fallback, &both_files, &rendered);
        let baseline = tracking_baseline(&fallback, &both_files, shown);

        assert!(
            tracking::estimate_tokens(baseline) >= tracking::estimate_tokens(shown),
            "baseline {} < shown {} would record negative savings",
            tracking::estimate_tokens(baseline),
            tracking::estimate_tokens(shown)
        );
    }

    #[test]
    fn test_tracking_baseline_identical_files_use_both_files() {
        let both_files = "a: 1\nb: 2\n\n---\na: 1\nb: 2\n";
        let shown = "[ok] Files are identical\n";

        assert_eq!(
            tracking_baseline("", both_files, shown),
            both_files,
            "identical files should still measure against the dump"
        );
    }

    #[test]
    fn test_tracking_baseline_empty_files_do_not_book_a_loss() {
        // Both files empty: the dump is shorter than the verdict line.
        let shown = "[ok] Files are identical\n";

        assert_eq!(tracking_baseline("", "\n---\n", shown), shown);
    }

    #[test]
    fn test_identical_files_keep_the_success_message() {
        let comparison = compare_files("same\n", "same\n");
        let rendered = "[ok] Files are identical\n";

        assert_eq!(
            select_file_diff_output(&comparison, "", "", rendered),
            rendered
        );
    }

    #[test]
    fn test_classic_diff_covers_modified_line_boundary_cases() {
        for (old, new) in [
            ("alpha beta gamma delta", "alpha beta XXXXX delta"),
            ("alpha beta gamma", "alpha beta"),
            ("alpha beta gamma delta", "XXXXX beta gamma delta"),
        ] {
            let diff = changes_of(&format!("{old}\n"), &format!("{new}\n"));
            let fallback = format_classic_diff(&diff);

            assert!(fallback.contains(&format!("< {old}")), "got:\n{fallback}");
            assert!(fallback.contains(&format!("> {new}")), "got:\n{fallback}");
        }
    }

    #[test]
    fn test_classic_diff_groups_a_replacement_after_a_shift() {
        // The `-`/`+` halves of a replacement carry different line numbers once
        // an insertion has shifted the two files apart. Grouping them by equal
        // line numbers degrades every replacement past the shift into a
        // separate `NdM` plus `NaM`: still well-formed, still wrong about what
        // changed together.
        let content1 = "keep\nalpha beta\n";
        let content2 = "INSERTED\nkeep\nzzzz yyyy\n";
        let diff = changes_of(content1, content2);
        let fallback = format_classic_diff(&diff);

        assert!(
            fallback.contains("2c3"),
            "replacement must group as one change hunk, got:\n{}",
            fallback
        );
        assert!(fallback.contains("< alpha beta"), "got:\n{}", fallback);
        assert!(fallback.contains("> zzzz yyyy"), "got:\n{}", fallback);
        // The insertion is anchored in file1 and ranged in file2.
        assert!(fallback.contains("0a1"), "got:\n{}", fallback);
    }

    #[test]
    fn test_classic_diff_anchors_a_deletion_in_file2() {
        // `NdM` names the file2 line the deleted text would have followed. With
        // one frame for both files that anchor drifts by every earlier
        // insertion.
        let content1 = "a\nGONE\nb\n";
        let content2 = "NEW\na\nb\n";
        let diff = changes_of(content1, content2);
        let fallback = format_classic_diff(&diff);

        assert!(fallback.contains("0a1"), "got:\n{}", fallback);
        assert!(fallback.contains("2d2"), "got:\n{}", fallback);
        assert!(fallback.contains("< GONE"), "got:\n{}", fallback);
    }

    #[test]
    fn test_over_cap_comparison_is_not_reported_as_identical() {
        // An empty change list is not a synonym for "identical". Every refusal
        // to build a listing produces one, and routing those through the
        // identical branch reports two wholly different files as the same, with
        // exit 0 — the bug this module exists to close.
        let content1: String = (0..60_000).map(|i| format!("a{}\n", i)).collect();
        let content2: String = (0..60_000).map(|i| format!("b{}\n", i)).collect();
        let both_files = format!("{}\n---\n{}", content1, content2);

        let comparison = compare_files(&content1, &content2);
        let fallback = classic_fallback(&comparison);
        let (rendered, code) = render_diff(Path::new("a.txt"), Path::new("b.txt"), &comparison);
        let shown = select_file_diff_output(&comparison, &fallback, &both_files, &rendered);

        assert_eq!(code, 1, "files that differ must exit 1");
        assert!(
            !shown.contains("identical"),
            "over-cap comparison reported as identical:\n{}",
            shown
        );
        assert!(shown.contains("lines differ"), "got:\n{}", shown);
        assert!(
            shown.len() < both_files.len(),
            "the refusal must not fall back to the dump"
        );
    }

    #[test]
    fn test_invisible_difference_is_not_reported_as_identical() {
        let comparison = compare_files("x\r\ny\r\n", "x\ny\n");
        let fallback = classic_fallback(&comparison);
        let (rendered, code) = render_diff(Path::new("a.txt"), Path::new("b.txt"), &comparison);

        assert_eq!(code, 1);
        assert!(fallback.is_empty(), "no classic diff exists here");
        assert_eq!(
            select_file_diff_output(&comparison, &fallback, "x\r\ny\r\n\n---\nx\ny\n", &rendered),
            rendered,
            "an affordable invisible-difference message survives the guard"
        );
    }

    // --- region grouping, pairing, script size, trace accounting, chrome ---

    #[test]
    fn test_classic_diff_groups_a_region_as_one_hunk() {
        // Two lines replaced by three. GNU prints one `2,3c2,4`; rendering the
        // pairing split it into a `2,3c2,3` plus a `3a4`, which asserts a
        // correspondence the classic format cannot carry.
        let diff = changes_of("ctx\nA\nB\nend\n", "ctx\nX\nY\nZ\nend\n");
        assert_eq!(
            format_classic_diff(&diff),
            "2,3c2,4\n< A\n< B\n---\n> X\n> Y\n> Z\n"
        );
    }

    #[test]
    fn test_classic_diff_is_independent_of_the_pairing() {
        // A rewrite next to a pure insertion in the same region: whichever new
        // line the `~` pairs with, the hunk is the region.
        let diff = changes_of("ctx\nvalue = 1\nend\n", "ctx\nvalue = 9\nvalue = 2\nend\n");
        assert_eq!(
            format_classic_diff(&diff),
            "2c2,3\n< value = 1\n---\n> value = 9\n> value = 2\n"
        );
    }

    #[test]
    fn test_pairing_is_by_similarity_not_position() {
        // An insertion at the head of the run shifted every positional pair,
        // so `total` was reported as rewritten into the inserted `offset` line
        // while its real rewrite listed as an unrelated addition. The threshold
        // cannot repair that — both candidates clear 0.5 — only the rule can.
        let old = "let total = compute(x);";
        let inserted = "let count = compute(z);";
        let rewritten = "let total = compute(x, y);";
        assert!(similarity(old, inserted) > REWRITE_SIMILARITY);
        assert!(similarity(old, inserted) < similarity(old, rewritten));

        let diff = changes_of(
            &format!("ctx\n{}\nend\n", old),
            &format!("ctx\n{}\n{}\nend\n", inserted, rewritten),
        );
        assert_eq!((diff.added, diff.removed, diff.modified), (1, 0, 1));
        let listed = format_diff_changes(&diff);
        assert!(
            listed.contains(&format!("~   2 {} → {}", old, rewritten)),
            "got:\n{}",
            listed
        );
        assert!(listed.contains(&format!("+   2 {}", inserted)), "got:\n{}", listed);
    }

    #[test]
    fn test_pairing_tie_breaks_toward_the_diagonal() {
        // `value = 1` scores 0.78 against both `value = 9` and `value = 2`:
        // a character-set metric cannot tell them apart, so the nearer position
        // decides and the result is stated as a pairing rule, not an accident.
        let pairs = pair_rewrites(
            &["value = 1".to_string()],
            &["value = 9".to_string(), "value = 2".to_string()],
        );
        assert_eq!(pairs, vec![(0, 0)]);
    }

    #[test]
    fn test_pairing_takes_the_best_score_first() {
        // Two deletions, two insertions, the better match for the first
        // deletion sitting second. Greedy-by-score pairs each with its own
        // rewrite; positional pairing would have crossed them.
        let pairs = pair_rewrites(
            &["let alpha = 1;".to_string(), "fn beta_gamma() {}".to_string()],
            &[
                "fn beta_gamma_delta() {}".to_string(),
                "let alpha = 2;".to_string(),
            ],
        );
        assert_eq!(pairs, vec![(0, 1), (1, 0)]);
    }

    #[test]
    fn test_pairing_leaves_dissimilar_lines_unpaired() {
        let pairs = pair_rewrites(&["aaaa".to_string()], &["zzzz".to_string()]);
        assert!(pairs.is_empty());
    }

    #[test]
    fn test_pairing_past_the_cell_cap_falls_back_to_position() {
        // 17 x 17 is over the cap. Every line pairs with its positional twin,
        // which is right for an in-place rewrite of a block.
        let old: Vec<String> = (0..17).map(|i| format!("line {} = {};", i, i)).collect();
        let new: Vec<String> = (0..17).map(|i| format!("line {} = {};", i, i + 1)).collect();
        assert!(old.len() * new.len() > PAIRING_CELL_CAP);
        let pairs = pair_rewrites(&old, &new);
        assert_eq!(pairs, (0..17).map(|i| (i, i)).collect::<Vec<_>>());
    }

    #[test]
    fn test_script_size_tracks_the_change_not_the_file() {
        // A million-line pair with three rewrites used to carry one `Keep` per
        // matched line — 45MB of separators. A run-length `Keep` makes the
        // script a handful of entries whatever the file's length.
        let a_lines: Vec<String> = (0..100_000).map(|i| format!("line {}", i)).collect();
        let mut b_lines = a_lines.clone();
        for i in [10, 50_000, 99_990] {
            b_lines[i] = format!("line {} EDITED", i);
        }
        let a: Vec<&str> = a_lines.iter().map(|s| s.as_str()).collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        // Trim leaves the middle from line 11 to line 99,991; the script over
        // it is 3 deletions, 3 insertions and the 2 matched runs between them.
        let Ok(Aligned::Script(ops)) = myers_ops(&a[10..99_991], &b[10..99_991]) else {
            panic!("three rewrites must align");
        };
        assert!(ops.len() <= 8, "script must not scale with the file, got {}", ops.len());
        let kept: usize = ops
            .iter()
            .map(|op| match op {
                Op::Keep(run) => *run,
                _ => 0,
            })
            .sum();
        assert_eq!(kept, 99_981 - 3, "every matched line is in exactly one run");

        let result = compute_diff(&a, &b);
        assert_eq!((result.added, result.removed, result.modified), (0, 0, 3));
    }

    #[test]
    fn test_trace_budget_charges_per_round_overhead() {
        // One line against 250,000 sharing a line in the middle: a three-slot
        // window per round, so the snapshots alone are 750,000 cells and would
        // fit. Each round also stores its first diagonal and its length, and
        // those are charged too, so the budget runs out around round 200,000 —
        // when the trace really is `MAX_TRACE_CELLS` `i32`s and not several
        // times that in per-round allocations.
        let b_lines: Vec<String> = (0..250_000)
            .map(|i| {
                if i == 125_000 {
                    "KEEP".to_string()
                } else {
                    format!("ins {}", i)
                }
            })
            .collect();
        let b: Vec<&str> = b_lines.iter().map(|s| s.as_str()).collect();

        match myers_ops(&["KEEP"], &b) {
            Err(d) => {
                let per_round = 3 + TRACE_ROUND_OVERHEAD;
                assert!(
                    d.abs_diff(MAX_TRACE_CELLS / per_round) <= 2,
                    "gave up at round {}, expected ~{}",
                    d,
                    MAX_TRACE_CELLS / per_round
                );
            }
            Ok(_) => panic!("250,000 rounds must exceed the trace budget"),
        }
    }

    #[test]
    fn test_render_has_no_header_and_no_blank_line() {
        // The framing cost more than the change list saved on agent-sized
        // diffs: the header echoed two paths the caller typed, the blank line
        // bought nothing. What is left is the counts line and the listing.
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "a\nb\n", "a\nNEW\nb\n");
        assert_eq!(code, 1);
        assert_eq!(out, "+   2 NEW\n");
    }

    #[test]
    fn test_render_beats_classic_diff_on_a_small_edit() {
        // The reason the chrome went: a one-line rewrite in a short file is the
        // diff an agent produces all day, and the condensed render has to win
        // it or `never_worse` hands back the classic diff every time.
        let content1: String = (0..40).map(|i| format!("line {} = {};\n", i, i)).collect();
        let content2 = content1.replace("line 20 = 20;", "line 20 = 21;");
        let comparison = compare_files(&content1, &content2);
        let fallback = classic_fallback(&comparison);
        let (rendered, _) = render_diff(Path::new("a.txt"), Path::new("b.txt"), &comparison);

        assert_eq!(rendered, "~  21 line 20 = 20; → line 20 = 21;\n");
        assert!(
            rendered.len() < fallback.len(),
            "condensed {} bytes must beat classic {} bytes:\n{}\n{}",
            rendered.len(),
            fallback.len(),
            rendered,
            fallback
        );
    }

    #[test]
    fn test_render_counts_line_appears_once_the_listing_is_long() {
        let content1: String = (0..100).map(|i| format!("line {}\n", i)).collect();
        let short = content1.replace("line 5\n", "line 5 EDITED\n");
        let (out, _) = render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &short);
        assert!(!out.contains("modified\n"), "one change needs no summary, got:\n{}", out);

        let mut long = content1.clone();
        for i in 0..COUNTS_MIN_LISTED_LINES {
            long = long.replace(&format!("line {}\n", i * 3), &format!("line {} EDITED\n", i * 3));
        }
        let (out, _) = render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &content1, &long);
        assert!(
            out.starts_with(&format!(
                "   +0 added, -0 removed, ~{} modified\n",
                COUNTS_MIN_LISTED_LINES
            )),
            "got:\n{}",
            out
        );
    }

    #[test]
    fn test_render_invisible_difference_is_one_line() {
        let (out, code) =
            render_file_diff(Path::new("a.txt"), Path::new("b.txt"), "x\n", "x\r\n");
        assert_eq!(code, 1);
        assert_eq!(
            out,
            "files differ only in whitespace or line endings (line endings: 0 CRLF vs 1 CRLF)\n"
        );
    }

    // --- render_file_diff (issue #2364 regression) ---

    #[test]
    fn test_render_modified_only_yaml_not_identical() {
        // "a: 1" vs "a: 2" is classified as modified (similarity > 0.5);
        // the identical check must not ignore modified-only diffs.
        let (out, code) = render_file_diff(
            Path::new("one.yaml"),
            Path::new("two.yaml"),
            "a: 1\n",
            "a: 2\n",
        );
        assert!(
            !out.contains("identical"),
            "modified-only diff reported as identical:\n{}",
            out
        );
        assert!(out.contains("~   1 a: 1 → a: 2"), "got:\n{}", out);
        assert_eq!(code, 1, "differing files must exit 1 (diff convention)");
    }

    #[test]
    fn test_render_crlf_difference_is_not_identical() {
        // `str::lines()` strips a trailing `\r`, so a CRLF-vs-LF file pair used
        // to collapse to identical line vectors and report "[ok] Files are
        // identical" with exit 0. `cmp` says these differ at byte 6.
        let (out, code) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "keep1\nkeep2\nkeep3\n",
            "keep1\r\nkeep2\r\nkeep3\n",
        );
        assert!(
            !out.contains("identical"),
            "CRLF-vs-LF reported as identical:\n{}",
            out
        );
        assert_eq!(code, 1, "differing files must exit 1");
        assert!(out.contains("0 CRLF vs 2 CRLF"), "got: {}", out);
    }

    #[test]
    fn test_render_trailing_newline_difference_is_not_identical() {
        // The other thing `lines()` normalizes: the final newline is optional.
        let (out, code) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "keep1\nkeep2\n",
            "keep1\nkeep2",
        );
        assert!(
            !out.contains("identical"),
            "trailing-newline diff reported as identical:\n{}",
            out
        );
        assert_eq!(code, 1);
        assert!(out.contains("trailing newline: present vs absent"), "got: {}", out);
    }

    #[test]
    fn test_render_byte_identical_is_identical() {
        // The guard must not flip the true-identity case to a false positive.
        let (out, code) = render_file_diff(
            Path::new("a.txt"),
            Path::new("b.txt"),
            "keep1\nkeep2\n",
            "keep1\nkeep2\n",
        );
        assert!(out.contains("[ok] Files are identical"));
        assert_eq!(code, 0);
    }

    #[test]
    fn test_render_partial_crlf_matches_reported_repro() {
        // The shape actually observed: a 200-line file where a Windows editor
        // touched 24 lines. Text identical, bytes differ, `cmp` exits 1.
        let plain: String = (0..200).map(|i| format!("line {} content here\n", i)).collect();
        let mixed: String = (0..200)
            .map(|i| {
                if (50..74).contains(&i) {
                    format!("line {} content here\r\n", i)
                } else {
                    format!("line {} content here\n", i)
                }
            })
            .collect();
        assert_ne!(plain, mixed, "fixture must actually differ");

        let (out, code) = render_file_diff(Path::new("a.txt"), Path::new("b.txt"), &plain, &mixed);
        assert!(!out.contains("identical"), "got: {}", out);
        assert_eq!(code, 1, "must exit 1 so a `diff` gate fails");
        assert!(out.contains("0 CRLF vs 24 CRLF"), "got: {}", out);
    }

    #[test]
    fn test_render_modified_only_json_not_identical() {
        let (out, code) = render_file_diff(
            Path::new("j1.json"),
            Path::new("j2.json"),
            "{\"a\": 1}\n",
            "{\"a\": 2}\n",
        );
        assert!(
            !out.contains("identical"),
            "modified-only diff reported as identical:\n{}",
            out
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn test_render_identical_files_exit_zero() {
        let (out, code) = render_file_diff(
            Path::new("a.yaml"),
            Path::new("b.yaml"),
            "a: 1\nb: 2\n",
            "a: 1\nb: 2\n",
        );
        assert!(out.contains("[ok] Files are identical"));
        assert_eq!(code, 0);
    }

    #[test]
    fn test_render_added_removed_exit_one() {
        let (out, code) = render_file_diff(Path::new("t1.txt"), Path::new("t2.txt"), "x\n", "y\n");
        assert!(out.contains("-   1 x\n"), "got:\n{}", out);
        assert!(out.contains("+   1 y\n"), "got:\n{}", out);
        assert_eq!(code, 1);
    }

    // --- condense_unified_diff ---

    #[test]
    fn test_condense_unified_diff_single_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
     println!("world");
 }
"#;
        let result = condense_unified_diff(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+1"));
        assert!(result.contains("println"));
    }

    #[test]
    fn test_condense_unified_diff_multiple_files() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
+added line
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
-removed line
"#;
        let result = condense_unified_diff(diff);
        assert!(result.contains("a.rs"));
        assert!(result.contains("b.rs"));
    }

    #[test]
    fn test_condense_unified_diff_markers_at_column_0() {
        // Indented markers make anchored greps (`^[+-]`) match nothing, so a
        // "was anything removed?" audit answers no while the content is there.
        //
        // Two files on purpose. A file's changes are flushed at two separate
        // sites: once per `+++` for the preceding file, once after the loop for
        // the last one. A single-file fixture only ever reaches the second, so
        // the first could be reverted with the whole suite still green.
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n";
        let result = condense_unified_diff(diff);
        for want in ["-fn old() {}", "+fn new() {}", "-let x = 1;", "+let x = 2;"] {
            assert!(
                result.lines().any(|l| l == want),
                "missing {want:?} at column 0 in:\n{}",
                result
            );
        }
        // Match on leading whitespace rather than a single space: the indent
        // this guards against is two spaces, so `" +"` / `" -"` would never
        // fire and the assertion would pass on the very code it rejects.
        assert!(
            !result.lines().any(|l| {
                let trimmed = l.trim_start();
                trimmed.len() != l.len()
                    && (trimmed.starts_with('+') || trimmed.starts_with('-'))
            }),
            "change lines must not be indented:\n{}",
            result
        );
    }

    #[test]
    fn test_condense_unified_diff_empty() {
        let result = condense_unified_diff("");
        assert!(result.is_empty());
    }

    // --- truncation accuracy ---

    fn make_large_unified_diff(added: usize, removed: usize) -> String {
        let mut lines = vec![
            "diff --git a/config.yaml b/config.yaml".to_string(),
            "--- a/config.yaml".to_string(),
            "+++ b/config.yaml".to_string(),
            "@@ -1,200 +1,200 @@".to_string(),
        ];
        for i in 0..removed {
            lines.push(format!("-old_value_{}", i));
        }
        for i in 0..added {
            lines.push(format!("+new_value_{}", i));
        }
        lines.join("\n")
    }

    #[test]
    fn test_condense_unified_diff_large_no_false_overflow_indicator() {
        // All 200 changes are shown in full (never truncate diff content).
        // No misleading "... +N more" should appear.
        let diff = make_large_unified_diff(100, 100);
        let result = condense_unified_diff(&diff);
        assert!(
            !result.contains("more"),
            "No overflow indicator expected when all lines are shown, got:\n{}",
            result
        );
        assert!(
            result.contains("+new_value_99"),
            "Last added line must be present (no truncation)"
        );
        assert!(
            result.contains("-old_value_99"),
            "Last removed line must be present (no truncation)"
        );
    }

    #[test]
    fn test_condense_unified_diff_no_false_overflow() {
        // Counter-case to the 200-change test above: no indicator at small sizes either.
        let diff = make_large_unified_diff(4, 4);
        let result = condense_unified_diff(&diff);
        assert!(
            !result.contains("more"),
            "No overflow message expected for 8 changes, got:\n{}",
            result
        );
    }

    #[test]
    fn test_no_truncation_large_diff() {
        // Verify compute_diff returns all changes without truncation
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..500 {
            a.push(format!("line_{}", i));
            if i % 3 == 0 {
                b.push(format!("CHANGED_{}", i));
            } else {
                b.push(format!("line_{}", i));
            }
        }
        let a_refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let b_refs: Vec<&str> = b.iter().map(|s| s.as_str()).collect();
        let result = compute_diff(&a_refs, &b_refs);

        assert!(
            result.changes().len() > 100,
            "Expected 100+ changes, got {}",
            result.changes().len()
        );
        assert!(!result.changes().is_empty());
    }

    #[test]
    fn test_format_diff_shows_all_changes() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..100 {
            a.push(format!("old_line_{}", i));
            b.push(format!("new_line_{}", i));
        }
        let a_refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let b_refs: Vec<&str> = b.iter().map(|s| s.as_str()).collect();
        let diff = compute_diff(&a_refs, &b_refs);
        let output = format_diff_changes(&diff);

        assert!(output.contains("old_line_0"), "should contain first change");
        assert!(output.contains("new_line_99"), "should contain last change");
    }

    #[test]
    fn test_long_lines_not_truncated() {
        let long_line = "x".repeat(500);
        let a = vec![long_line.as_str()];
        let b = vec!["short"];
        let result = compute_diff(&a, &b);
        match &result.changes()[0] {
            DiffChange::Removed { text, .. } | DiffChange::Added { text, .. } => {
                assert_eq!(text.len(), 500, "Line was truncated!");
            }
            DiffChange::Modified { old, .. } => {
                assert_eq!(old.len(), 500, "Line was truncated!");
            }
        }
    }
}

#![allow(dead_code)] // Semantic output model is consumed by later adapter tasks.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetClass {
    Acknowledgement,
    State,
    Collection,
    Diagnostic,
    Source,
}

impl BudgetClass {
    pub const fn max_tokens(self) -> usize {
        match self {
            Self::Acknowledgement => 128,
            Self::State => 512,
            Self::Collection => 1_024,
            Self::Diagnostic => 2_048,
            Self::Source => 4_096,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledgement => "acknowledgement",
            Self::State => "state",
            Self::Collection => "collection",
            Self::Diagnostic => "diagnostic",
            Self::Source => "source",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactReason {
    Structured,
    Interactive,
    Binary,
    Streaming,
    Unknown,
    Sensitive,
}

impl ExactReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Structured => "structured",
            Self::Interactive => "interactive",
            Self::Binary => "binary",
            Self::Streaming => "streaming",
            Self::Unknown => "unknown",
            Self::Sensitive => "sensitive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputContract {
    AiOwned(BudgetClass),
    Exact(ExactReason),
    Legacy,
}

impl OutputContract {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AiOwned(_) => "ai_owned",
            Self::Exact(_) => "exact",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Success,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Success => "success",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiRecord {
    pub severity: Severity,
    pub text: String,
    pub group: Option<String>,
    source_order: usize,
    represented_items: usize,
    omitted_items: usize,
}

impl AiRecord {
    pub fn new(severity: Severity, text: impl Into<String>) -> Self {
        Self {
            severity,
            text: text.into(),
            group: None,
            source_order: 0,
            represented_items: 1,
            omitted_items: 0,
        }
    }

    pub fn grouped(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn representing(mut self, items: usize) -> Self {
        self.represented_items = items.max(1);
        self
    }

    /// Marks source items compacted inside this record. They count as omitted
    /// only when this record is emitted; a budget-dropped record is instead
    /// accounted for by its full represented-item count.
    pub fn omitting(mut self, items: usize) -> Self {
        self.omitted_items = items;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Omission {
    pub items: usize,
    pub groups: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DocumentBody {
    Semantic,
    Legacy(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiDocument {
    status: Option<String>,
    facts: Vec<(String, String)>,
    records: Vec<AiRecord>,
    body: DocumentBody,
    declared_omission: Option<Omission>,
    parser_failed: bool,
    lossless_baseline: Option<String>,
}

impl AiDocument {
    pub fn new(status: Option<impl Into<String>>) -> Self {
        Self {
            status: status.map(Into::into),
            facts: Vec::new(),
            records: Vec::new(),
            body: DocumentBody::Semantic,
            declared_omission: None,
            parser_failed: false,
            lossless_baseline: None,
        }
    }

    pub fn legacy(raw: impl Into<String>) -> Self {
        Self {
            status: None,
            facts: Vec::new(),
            records: Vec::new(),
            body: DocumentBody::Legacy(raw.into()),
            declared_omission: None,
            parser_failed: false,
            lossless_baseline: None,
        }
    }

    pub fn parse_failure(raw: &str, error: &str) -> Self {
        const EDGE_LINES: usize = 10;
        let lines: Vec<&str> = raw.lines().collect();
        let mut doc = Self::new(Some("error"));
        doc.fact("filter", "parse-failed");
        doc.fact(
            "detail",
            error.split_whitespace().collect::<Vec<_>>().join("_"),
        );
        doc.parser_failed = true;

        if lines.len() <= EDGE_LINES * 2 {
            for line in lines {
                doc.push(AiRecord::new(Severity::Error, line));
            }
            return doc;
        }
        for line in &lines[..EDGE_LINES] {
            doc.push(AiRecord::new(Severity::Error, *line));
        }
        for line in &lines[lines.len() - EDGE_LINES..] {
            doc.push(AiRecord::new(Severity::Error, *line));
        }
        doc.with_omission(Omission {
            items: lines.len() - EDGE_LINES * 2,
            groups: 0,
        })
    }

    pub fn fact(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.facts.push((key.into(), value.into()));
    }

    pub fn push(&mut self, mut record: AiRecord) {
        record.source_order = self.records.len();
        self.records.push(record);
    }

    pub fn with_omission(mut self, omission: Omission) -> Self {
        self.declared_omission = Some(omission);
        self
    }

    /// Supplies the native-equivalent stdout used for no-worse fallback,
    /// tracking, and lossless recovery when an adapter needed parse aids.
    pub fn with_lossless_baseline(mut self, baseline: impl Into<String>) -> Self {
        self.lossless_baseline = Some(baseline.into());
        self
    }

    pub(crate) fn lossless_baseline(&self) -> Option<&str> {
        self.lossless_baseline.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedOutput {
    pub text: String,
    pub omission: Option<Omission>,
    pub parser_failed: bool,
}

pub enum PreparedEmission {
    Plain {
        output: String,
        meta: EmissionMeta,
    },
    Recovered {
        commit: crate::core::tee::LosslessTeeCommit,
        meta: EmissionMeta,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmissionMeta {
    pub omitted_items: usize,
    pub omitted_groups: usize,
    pub recovery_created: bool,
    pub parser_failed: bool,
    pub used_raw_fallback: bool,
    /// Stable runtime error kind, when emission could not complete its normal
    /// recovery/filtering contract. Kept as a string literal so it can be
    /// persisted without allocating on the hot path.
    pub runtime_error: Option<&'static str>,
}

impl PreparedEmission {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Plain { output, .. } => output,
            Self::Recovered { commit, .. } => std::str::from_utf8(commit.as_bytes())
                .expect("lossless tee commit output is owned UTF-8"),
        }
    }

    pub fn meta(&self) -> EmissionMeta {
        match self {
            Self::Plain { meta, .. } | Self::Recovered { meta, .. } => *meta,
        }
    }

    pub fn recovery_created(&self) -> bool {
        self.meta().recovery_created
    }
}

pub fn prepare_emission(
    raw: &str,
    command_slug: &str,
    rendered: RenderedOutput,
    trailing_newline: bool,
) -> PreparedEmission {
    prepare_emission_with_baseline(raw, raw, command_slug, rendered, trailing_newline)
}

pub fn prepare_emission_with_baseline(
    raw: &str,
    fallback_baseline: &str,
    command_slug: &str,
    rendered: RenderedOutput,
    trailing_newline: bool,
) -> PreparedEmission {
    prepare_emission_with_fallback(
        raw,
        fallback_baseline,
        command_slug,
        rendered,
        trailing_newline,
        crate::core::tee::reserve_lossless_tee_for_emission,
    )
}

fn prepare_emission_with<F>(
    raw: &str,
    command_slug: &str,
    rendered: RenderedOutput,
    trailing_newline: bool,
    reserve: F,
) -> PreparedEmission
where
    F: FnOnce(
        &str,
        &str,
    ) -> Result<
        crate::core::tee::LosslessTeeReservation,
        crate::core::tee::LosslessTeeReservationError,
    >,
{
    prepare_emission_with_fallback(raw, raw, command_slug, rendered, trailing_newline, reserve)
}

fn prepare_emission_with_fallback<F>(
    raw: &str,
    fallback_baseline: &str,
    command_slug: &str,
    rendered: RenderedOutput,
    trailing_newline: bool,
    reserve: F,
) -> PreparedEmission
where
    F: FnOnce(
        &str,
        &str,
    ) -> Result<
        crate::core::tee::LosslessTeeReservation,
        crate::core::tee::LosslessTeeReservationError,
    >,
{
    let parser_failed = rendered.parser_failed;
    let parser_error = parser_failed.then_some("filter_failed");
    let raw_fallback = frame_payload(fallback_baseline, trailing_newline);
    let Some(omission) = rendered.omission else {
        let candidate = frame_payload(&rendered.text, trailing_newline);
        let used_raw_fallback = crate::core::tracking::estimate_tokens(&candidate)
            > crate::core::tracking::estimate_tokens(&raw_fallback);
        let output = if used_raw_fallback {
            raw_fallback
        } else {
            candidate
        };
        return PreparedEmission::Plain {
            output,
            meta: EmissionMeta {
                parser_failed,
                used_raw_fallback,
                runtime_error: parser_error,
                ..EmissionMeta::default()
            },
        };
    };

    let reservation = match reserve(raw, command_slug) {
        Ok(reservation) => reservation,
        Err(crate::core::tee::LosslessTeeReservationError::Oversized) => {
            let body = rendered.text.trim_end_matches(['\r', '\n']);
            let compact = frame_payload(
                &format!(
                    "{body}\nomitted items={} groups={} recovery=unavailable",
                    omission.items, omission.groups
                ),
                trailing_newline,
            );
            if strictly_smaller(&raw_fallback, &compact) {
                return PreparedEmission::Plain {
                    output: compact,
                    meta: EmissionMeta {
                        omitted_items: omission.items,
                        omitted_groups: omission.groups,
                        parser_failed,
                        runtime_error: Some("oversized_output_recovery_unavailable"),
                        ..EmissionMeta::default()
                    },
                };
            }
            return PreparedEmission::Plain {
                output: raw_fallback,
                meta: EmissionMeta {
                    parser_failed,
                    used_raw_fallback: true,
                    runtime_error: Some("oversized_output_recovery_unavailable"),
                    ..EmissionMeta::default()
                },
            };
        }
        Err(crate::core::tee::LosslessTeeReservationError::Unavailable) => {
            return PreparedEmission::Plain {
                output: raw_fallback,
                meta: EmissionMeta {
                    parser_failed,
                    used_raw_fallback: true,
                    runtime_error: Some("recovery_unavailable"),
                    ..EmissionMeta::default()
                },
            };
        }
    };
    let recovery = reservation.recovery_command();
    let body = rendered.text.trim_end_matches(['\r', '\n']);
    let candidate = frame_payload(
        &format!(
            "{body}\nomitted items={} groups={} recover={recovery}",
            omission.items, omission.groups
        ),
        trailing_newline,
    );
    let meta = EmissionMeta {
        omitted_items: omission.items,
        omitted_groups: omission.groups,
        recovery_created: true,
        parser_failed,
        used_raw_fallback: false,
        runtime_error: parser_error,
    };
    match reservation.commit_output_if_better(&raw_fallback, candidate) {
        Some(commit) => PreparedEmission::Recovered { commit, meta },
        None => PreparedEmission::Plain {
            output: raw_fallback,
            meta: EmissionMeta {
                parser_failed,
                used_raw_fallback: true,
                runtime_error: parser_error,
                ..EmissionMeta::default()
            },
        },
    }
}

pub(crate) fn frame_payload(output: &str, trailing_newline: bool) -> String {
    let mut framed = output.to_string();
    if trailing_newline && !framed.ends_with('\n') {
        framed.push('\n');
    }
    framed
}

pub(crate) fn strictly_smaller(raw: &str, output: &str) -> bool {
    crate::core::tracking::estimate_tokens(output) < crate::core::tracking::estimate_tokens(raw)
}

#[derive(Debug)]
struct CollapsedRecord {
    group: Option<String>,
    text: String,
    source_records: usize,
    represented_items: usize,
    omitted_items: usize,
}

pub fn render(document: &AiDocument, budget: BudgetClass) -> RenderedOutput {
    render_with_max_tokens(document, budget, None)
}

/// Render a semantic document with an optional request-level token limit.
/// Legacy documents remain untouched because their output contract is exact.
pub fn render_with_max_tokens(
    document: &AiDocument,
    budget: BudgetClass,
    max_tokens: Option<usize>,
) -> RenderedOutput {
    let max_tokens = max_tokens.unwrap_or_else(|| budget.max_tokens());
    match &document.body {
        DocumentBody::Legacy(text) => RenderedOutput {
            text: text.clone(),
            omission: document.declared_omission.clone(),
            parser_failed: document.parser_failed,
        },
        DocumentBody::Semantic => render_semantic(document, max_tokens),
    }
}

fn render_semantic(document: &AiDocument, max_tokens: usize) -> RenderedOutput {
    let records = collapsed_records(document);
    let (mut lines, omitted_summary_items) = summary_lines(document, max_tokens);

    let mut emitted = 0;
    for record in &records {
        let line = if record.source_records > 1 {
            format!("{} repeats={}", record.text, record.source_records)
        } else {
            record.text.clone()
        };
        let mut candidate = lines.clone();
        candidate.push(line.clone());
        if estimate_joined_tokens(&candidate) > max_tokens {
            break;
        }
        lines.push(line);
        emitted += 1;
    }

    let emitted_internal_omissions = records[..emitted]
        .iter()
        .map(|record| record.omitted_items)
        .sum();
    let omission = omission_from(
        document.declared_omission.clone(),
        omitted_summary_items,
        emitted_internal_omissions,
        &records[emitted..],
    );
    if lines.is_empty() {
        if let Some(omission) = &omission {
            let line = format!(
                "omitted items={} groups={}",
                omission.items, omission.groups
            );
            if crate::core::tracking::estimate_tokens(&line) <= max_tokens {
                lines.push(line);
            }
        }
    }

    RenderedOutput {
        text: lines.join("\n"),
        omission,
        parser_failed: document.parser_failed,
    }
}

fn summary_lines(document: &AiDocument, max_tokens: usize) -> (Vec<String>, usize) {
    let mut fields = Vec::new();
    if let Some(status) = &document.status {
        fields.push(format!("status={status}"));
    }
    fields.extend(
        document
            .facts
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );

    if fields.is_empty() {
        return (Vec::new(), 0);
    }

    let mut emitted_fields: Vec<String> = Vec::new();
    let mut omitted_items = 0;
    for field in fields {
        let mut candidate = emitted_fields.clone();
        candidate.push(field.clone());
        if crate::core::tracking::estimate_tokens(&candidate.join(" ")) <= max_tokens {
            emitted_fields.push(field);
        } else {
            omitted_items += 1;
        }
    }

    let lines = if emitted_fields.is_empty() {
        Vec::new()
    } else {
        vec![emitted_fields.join(" ")]
    };
    (lines, omitted_items)
}

fn collapsed_records(document: &AiDocument) -> Vec<CollapsedRecord> {
    let mut records = document.records.clone();
    records.sort_by_key(|record| (record.severity, record.source_order));

    let mut collapsed: Vec<CollapsedRecord> = Vec::new();
    for record in records {
        if let Some(existing) = collapsed
            .iter_mut()
            .find(|existing| existing.group == record.group && existing.text == record.text)
        {
            existing.source_records += 1;
            existing.represented_items += record.represented_items;
            existing.omitted_items += record.omitted_items;
            continue;
        }

        collapsed.push(CollapsedRecord {
            group: record.group,
            text: record.text,
            source_records: 1,
            represented_items: record.represented_items,
            omitted_items: record.omitted_items,
        });
    }

    collapsed
}

fn omission_from(
    declared_omission: Option<Omission>,
    omitted_summary_items: usize,
    emitted_internal_omissions: usize,
    omitted_records: &[CollapsedRecord],
) -> Option<Omission> {
    let mut omitted = match declared_omission {
        Some(mut declared) => {
            declared.items += omitted_summary_items + emitted_internal_omissions;
            declared
        }
        None => Omission {
            items: omitted_summary_items + emitted_internal_omissions,
            groups: 0,
        },
    };
    let mut omitted_groups = std::collections::BTreeSet::new();

    for record in omitted_records {
        omitted.items += record.represented_items;
        if let Some(group) = &record.group {
            omitted_groups.insert(group);
        }
    }
    omitted.groups += omitted_groups.len();

    (omitted.items > 0 || omitted.groups > 0).then_some(omitted)
}

fn estimate_joined_tokens(lines: &[String]) -> usize {
    crate::core::tracking::estimate_tokens(&lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_limits_match_the_product_contract() {
        assert_eq!(BudgetClass::Acknowledgement.max_tokens(), 128);
        assert_eq!(BudgetClass::State.max_tokens(), 512);
        assert_eq!(BudgetClass::Collection.max_tokens(), 1_024);
        assert_eq!(BudgetClass::Diagnostic.max_tokens(), 2_048);
        assert_eq!(BudgetClass::Source.max_tokens(), 4_096);
    }

    #[test]
    fn explicit_semantic_token_limit_overrides_budget_class() {
        let mut doc = AiDocument::new(Some("ok"));
        for index in 0..300 {
            doc.push(AiRecord::new(
                Severity::Info,
                format!("src/generated/{index:03}.rs match=value"),
            ));
        }

        let default = render(&doc, BudgetClass::Source);
        let constrained = render_with_max_tokens(&doc, BudgetClass::Source, Some(64));

        assert!(crate::core::tracking::estimate_tokens(&default.text) > 64);
        assert!(crate::core::tracking::estimate_tokens(&constrained.text) <= 64);
        assert!(constrained.omission.is_some());
    }

    #[test]
    fn unknown_exact_reason_is_stable_for_tracking() {
        assert_eq!(ExactReason::Unknown.as_str(), "unknown");
        assert_eq!(
            OutputContract::Exact(ExactReason::Unknown).as_str(),
            "exact"
        );
    }

    #[test]
    fn stable_labels_cover_the_output_contract_vocabulary() {
        assert_eq!(
            [
                BudgetClass::Acknowledgement.as_str(),
                BudgetClass::State.as_str(),
                BudgetClass::Collection.as_str(),
                BudgetClass::Diagnostic.as_str(),
                BudgetClass::Source.as_str(),
            ],
            [
                "acknowledgement",
                "state",
                "collection",
                "diagnostic",
                "source"
            ]
        );
        assert_eq!(
            [
                ExactReason::Structured.as_str(),
                ExactReason::Interactive.as_str(),
                ExactReason::Binary.as_str(),
                ExactReason::Streaming.as_str(),
                ExactReason::Unknown.as_str(),
                ExactReason::Sensitive.as_str(),
            ],
            [
                "structured",
                "interactive",
                "binary",
                "streaming",
                "unknown",
                "sensitive"
            ]
        );
        assert_eq!(
            [
                OutputContract::AiOwned(BudgetClass::State).as_str(),
                OutputContract::Exact(ExactReason::Unknown).as_str(),
                OutputContract::Legacy.as_str(),
            ],
            ["ai_owned", "exact", "legacy"]
        );
        assert_eq!(
            [
                Severity::Error.as_str(),
                Severity::Warning.as_str(),
                Severity::Info.as_str(),
                Severity::Success.as_str(),
            ],
            ["error", "warning", "info", "success"]
        );
    }

    #[test]
    fn semantic_render_orders_failures_and_counts_duplicates() {
        let mut doc = AiDocument::new(Some("fail"));
        doc.fact("passed", "12");
        doc.push(AiRecord::new(Severity::Warning, "src/a.rs:2 W unused"));
        doc.push(AiRecord::new(
            Severity::Error,
            "src/b.rs:7 E0308 expected=u32 actual=String",
        ));
        doc.push(AiRecord::new(
            Severity::Error,
            "src/b.rs:7 E0308 expected=u32 actual=String",
        ));

        let rendered = render(&doc, BudgetClass::Diagnostic);

        assert_eq!(
            rendered.text,
            "status=fail passed=12\nsrc/b.rs:7 E0308 expected=u32 actual=String repeats=2\nsrc/a.rs:2 W unused"
        );
        assert_eq!(rendered.omission, None);
    }

    #[test]
    fn semantic_render_keeps_source_order_for_distinct_same_severity_records() {
        let mut doc = AiDocument::new(None::<String>);
        doc.push(AiRecord::new(Severity::Warning, "src/z.rs:9 W later_path"));
        doc.push(AiRecord::new(
            Severity::Warning,
            "src/a.rs:1 W earlier_path",
        ));

        let rendered = render(&doc, BudgetClass::Diagnostic);

        assert_eq!(
            rendered.text,
            "src/z.rs:9 W later_path\nsrc/a.rs:1 W earlier_path"
        );
        assert_eq!(rendered.omission, None);
    }

    #[test]
    fn semantic_render_reports_over_budget_summary_omission() {
        let mut doc = AiDocument::new(Some("x".repeat(600)));
        doc.fact("detail", "y".repeat(600));

        let rendered = render(&doc, BudgetClass::Acknowledgement);

        assert_eq!(rendered.text, "omitted items=2 groups=0");
        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 2,
                groups: 0
            })
        );
        assert!(
            crate::core::tracking::estimate_tokens(&rendered.text)
                <= BudgetClass::Acknowledgement.max_tokens()
        );
    }

    #[test]
    fn semantic_render_stops_before_collection_budget() {
        let mut doc = AiDocument::new(Some("ok"));
        for index in 0..300 {
            doc.push(AiRecord::new(
                Severity::Info,
                format!("src/generated/{index:03}.rs match=value"),
            ));
        }

        let rendered = render(&doc, BudgetClass::Collection);

        assert!(crate::core::tracking::estimate_tokens(&rendered.text) <= 1_024);
        assert!(rendered.omission.as_ref().is_some_and(|o| o.items > 0));
    }

    #[test]
    fn semantic_render_counts_omitted_items_and_distinct_groups() {
        let mut doc = AiDocument::new(None::<String>);
        doc.push(AiRecord::new(Severity::Info, long_record("src/a.rs")).grouped("alpha"));
        doc.push(AiRecord::new(Severity::Info, long_record("src/b.rs")).grouped("alpha"));
        doc.push(AiRecord::new(Severity::Info, long_record("src/c.rs")).grouped("beta"));

        let rendered = render(&doc, BudgetClass::Acknowledgement);

        assert_eq!(rendered.text, long_record("src/a.rs"));
        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 2,
                groups: 2
            })
        );
    }

    #[test]
    fn semantic_render_counts_logical_items_for_omitted_group_records() {
        let mut doc = AiDocument::new(None::<String>);
        doc.push(
            AiRecord::new(Severity::Info, format!("first {}", "x".repeat(260)))
                .grouped("alpha")
                .representing(4),
        );
        doc.push(
            AiRecord::new(Severity::Info, format!("second {}", "y".repeat(260)))
                .grouped("beta")
                .representing(12),
        );

        let rendered = render(&doc, BudgetClass::Acknowledgement);

        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 12,
                groups: 1,
            })
        );
    }

    #[test]
    fn semantic_render_adds_declared_omission_to_budget_omission() {
        let mut doc = AiDocument::new(None::<String>).with_omission(Omission {
            items: 5,
            groups: 1,
        });
        doc.push(AiRecord::new(Severity::Info, long_record("src/a.rs")).grouped("alpha"));
        doc.push(AiRecord::new(Severity::Info, long_record("src/b.rs")).grouped("beta"));

        let rendered = render(&doc, BudgetClass::Acknowledgement);

        assert_eq!(rendered.text, long_record("src/a.rs"));
        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 6,
                groups: 2
            })
        );
    }

    #[test]
    fn legacy_render_is_byte_compatible() {
        let raw = "native heading\n  native spacing\n";
        let rendered = render(&AiDocument::legacy(raw), BudgetClass::State);
        assert_eq!(rendered.text, raw);
        assert_eq!(rendered.omission, None);
    }

    #[test]
    fn legacy_render_carries_declared_omission_without_changing_text() {
        let raw = "native heading\n  native spacing\n";
        let rendered = render(
            &AiDocument::legacy(raw).with_omission(Omission {
                items: 7,
                groups: 3,
            }),
            BudgetClass::Acknowledgement,
        );

        assert_eq!(rendered.text, raw);
        assert_eq!(
            rendered.omission,
            Some(Omission {
                items: 7,
                groups: 3
            })
        );
    }

    #[test]
    fn final_newline_is_part_of_the_strict_token_comparison() {
        let raw = "12345";
        let unframed = "abcd";
        assert!(strictly_smaller(raw, unframed));

        let framed = frame_payload(unframed, true);

        assert_eq!(framed, "abcd\n");
        assert!(!strictly_smaller(raw, &framed));
    }

    #[test]
    fn prepared_plain_output_owns_its_final_framing() {
        let raw = "native output is substantially longer";
        let rendered = RenderedOutput {
            text: "short".to_string(),
            omission: None,
            parser_failed: false,
        };

        let prepared = prepare_emission_with(raw, "test", rendered, true, |_, _| {
            Err(crate::core::tee::LosslessTeeReservationError::Unavailable)
        });

        assert_eq!(prepared.as_str(), "short\n");
    }

    #[test]
    fn prepared_emission_keeps_required_baseline_when_ai_output_is_longer() {
        let raw = "visible.txt";
        let baseline = "visible.txt\n(1 filtered by policy)";
        let rendered = RenderedOutput {
            text: "status=inventory files=1 dirs=1\nvisible.txt\n(1 filtered by policy)"
                .to_string(),
            omission: None,
            parser_failed: false,
        };

        let prepared = prepare_emission_with_baseline(raw, baseline, "find", rendered, true);

        assert_eq!(prepared.as_str(), "visible.txt\n(1 filtered by policy)\n");
        assert!(prepared.meta().used_raw_fallback);
    }

    #[test]
    fn lossy_emission_contains_exact_counts_and_recovery_command() {
        let temp = tempfile::tempdir().unwrap();
        let rendered = RenderedOutput {
            text: "status=fail\nsrc/a.rs:1 E failure".to_string(),
            omission: Some(Omission {
                items: 14,
                groups: 3,
            }),
            parser_failed: false,
        };
        let raw = "native line\n".repeat(400);
        let prepared = prepare_emission_with(&raw, "cargo test", rendered, true, |raw, slug| {
            crate::core::tee::reserve_lossless_tee_file(raw, slug, temp.path(), 64_000, 20)
                .ok_or(crate::core::tee::LosslessTeeReservationError::Unavailable)
        });
        let shown = prepared.as_str();
        assert!(shown.contains("omitted items=14 groups=3 recover=rtk read -l none "));
        assert!(shown.ends_with('\n'));
        assert!(prepared.recovery_created());
    }

    #[test]
    fn lossy_emission_falls_back_to_raw_when_tee_is_disabled() {
        let raw = "full native output\n".repeat(100);
        let rendered = RenderedOutput {
            text: "short".to_string(),
            omission: Some(Omission {
                items: 99,
                groups: 1,
            }),
            parser_failed: false,
        };
        let prepared = prepare_emission_with(&raw, "test", rendered, true, |_, _| {
            Err(crate::core::tee::LosslessTeeReservationError::Unavailable)
        });
        assert_eq!(prepared.as_str(), raw);
        assert!(!prepared.recovery_created());
    }

    #[test]
    fn oversized_lossy_emission_stays_compact_without_unbounded_recovery() {
        let raw = "native output 漢字🙂\n".repeat(1_000);
        let rendered = RenderedOutput {
            text: "short".to_string(),
            omission: Some(Omission {
                items: 1,
                groups: 0,
            }),
            parser_failed: false,
        };

        let prepared = prepare_emission_with(&raw, "rg", rendered, true, |_, _| {
            Err(crate::core::tee::LosslessTeeReservationError::Oversized)
        });

        assert_eq!(
            prepared.as_str(),
            "short\nomitted items=1 groups=0 recovery=unavailable\n"
        );
        assert!(!prepared.recovery_created());
        assert!(!prepared.meta().used_raw_fallback);
        assert_eq!(
            prepared.meta().runtime_error,
            Some("oversized_output_recovery_unavailable")
        );
    }

    fn long_record(path: &str) -> String {
        format!("{path}:1 match={}", "v".repeat(260))
    }
}

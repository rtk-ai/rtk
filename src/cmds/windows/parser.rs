//! Span-preserving parsing for CMD command expressions.
#![allow(dead_code)] // The span model intentionally retains parser metadata for later adapters.

/// A byte range in the original command expression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A command segment that can be substituted by its source span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SimpleSegment {
    /// The trimmed source range for the entire command segment.
    pub span: Span,
    /// The command word, excluding a leading CMD `@` prefix.
    pub command_span: Span,
}

/// CMD operators observed at top level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorKind {
    Sequence,
    LineBreak,
    And,
    Or,
    Pipe,
    RedirectOutput,
    RedirectAppend,
    RedirectInput,
    OpenParen,
    CloseParen,
}

/// A top-level operator and its source range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Operator {
    pub kind: OperatorKind,
    pub span: Span,
}

/// A reason an expression must run unchanged through CMD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpaqueReason {
    OutputPipeline,
    OutputRedirection,
    ControlGroup,
    ControlCommand,
    BatchInvocation,
    DelayedExpansion,
    DriveChange,
    MalformedInput,
}

/// Parsed CMD expression. `opaque_reason` means no segments may be rewritten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedExpression {
    pub segments: Vec<SimpleSegment>,
    pub operators: Vec<Operator>,
    pub opaque_reason: Option<OpaqueReason>,
}

/// Parse a CMD expression without changing any source text.
///
/// The lexer intentionally fails open: ambiguous control flow and output-consuming
/// constructs are reported as opaque so their source can be passed straight to CMD.
pub fn parse_expression(source: &str) -> ParsedExpression {
    let bytes = source.as_bytes();
    let mut segments = Vec::new();
    let mut operators = Vec::new();
    let mut opaque_reason = None;
    let mut segment_start = 0;
    let mut in_quotes = false;
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'^' => {
                if index + 1 == bytes.len() {
                    opaque_reason.get_or_insert(OpaqueReason::MalformedInput);
                    index += 1;
                } else {
                    index += 2;
                }
            }
            b'"' => {
                in_quotes = !in_quotes;
                index += 1;
            }
            b'!' => {
                opaque_reason.get_or_insert(OpaqueReason::DelayedExpansion);
                index += 1;
            }
            b'\r' if !in_quotes && bytes.get(index + 1) == Some(&b'\n') => {
                push_segment(source, segment_start, index, &mut segments);
                operators.push(Operator {
                    kind: OperatorKind::LineBreak,
                    span: Span {
                        start: index,
                        end: index + 2,
                    },
                });
                segment_start = index + 2;
                index += 2;
            }
            b'&' if !in_quotes => {
                if trimmed_span(source, segment_start, index).is_none() {
                    opaque_reason.get_or_insert(OpaqueReason::MalformedInput);
                }
                push_segment(source, segment_start, index, &mut segments);
                let end = if bytes.get(index + 1) == Some(&b'&') {
                    index + 2
                } else {
                    index + 1
                };
                operators.push(Operator {
                    kind: if end == index + 2 {
                        OperatorKind::And
                    } else {
                        OperatorKind::Sequence
                    },
                    span: Span { start: index, end },
                });
                segment_start = end;
                index = end;
            }
            b'|' if !in_quotes => {
                if trimmed_span(source, segment_start, index).is_none() {
                    opaque_reason.get_or_insert(OpaqueReason::MalformedInput);
                }
                push_segment(source, segment_start, index, &mut segments);
                let end = if bytes.get(index + 1) == Some(&b'|') {
                    index + 2
                } else {
                    opaque_reason.get_or_insert(OpaqueReason::OutputPipeline);
                    index + 1
                };
                operators.push(Operator {
                    kind: if end == index + 2 {
                        OperatorKind::Or
                    } else {
                        OperatorKind::Pipe
                    },
                    span: Span { start: index, end },
                });
                segment_start = end;
                index = end;
            }
            b'>' if !in_quotes => {
                push_segment(source, segment_start, index, &mut segments);
                let end = if bytes.get(index + 1) == Some(&b'>') {
                    index + 2
                } else {
                    index + 1
                };
                operators.push(Operator {
                    kind: if end == index + 2 {
                        OperatorKind::RedirectAppend
                    } else {
                        OperatorKind::RedirectOutput
                    },
                    span: Span { start: index, end },
                });
                opaque_reason.get_or_insert(OpaqueReason::OutputRedirection);
                segment_start = end;
                index = end;
            }
            b'<' if !in_quotes => {
                push_segment(source, segment_start, index, &mut segments);
                let end = index + 1;
                operators.push(Operator {
                    kind: OperatorKind::RedirectInput,
                    span: Span { start: index, end },
                });
                segment_start = end;
                index = end;
            }
            b'(' if !in_quotes => {
                operators.push(Operator {
                    kind: OperatorKind::OpenParen,
                    span: Span {
                        start: index,
                        end: index + 1,
                    },
                });
                opaque_reason.get_or_insert(OpaqueReason::ControlGroup);
                index += 1;
            }
            b')' if !in_quotes => {
                operators.push(Operator {
                    kind: OperatorKind::CloseParen,
                    span: Span {
                        start: index,
                        end: index + 1,
                    },
                });
                opaque_reason.get_or_insert(OpaqueReason::ControlGroup);
                index += 1;
            }
            _ => index += 1,
        }
    }

    if matches!(
        operators.last().map(|operator| operator.kind),
        Some(OperatorKind::Sequence | OperatorKind::And | OperatorKind::Or)
    ) && trimmed_span(source, segment_start, source.len()).is_none()
    {
        opaque_reason.get_or_insert(OpaqueReason::MalformedInput);
    }
    push_segment(source, segment_start, source.len(), &mut segments);

    if in_quotes {
        opaque_reason.get_or_insert(OpaqueReason::MalformedInput);
    }
    if opaque_reason.is_none() {
        opaque_reason = segments
            .iter()
            .find_map(|segment| opaque_segment_reason(source, *segment));
    }

    ParsedExpression {
        segments,
        operators,
        opaque_reason,
    }
}

fn push_segment(source: &str, start: usize, end: usize, segments: &mut Vec<SimpleSegment>) {
    let Some(span) = trimmed_span(source, start, end) else {
        return;
    };
    let command_start = if source.as_bytes()[span.start] == b'@' {
        span.start + 1
    } else {
        span.start
    };
    let command_end = if command_start == span.end {
        command_start
    } else if source.as_bytes()[command_start] == b'"' {
        quoted_command_end(source, command_start, span.end)
    } else {
        source[command_start..span.end]
            .char_indices()
            .find_map(|(offset, character)| {
                character.is_whitespace().then_some(command_start + offset)
            })
            .unwrap_or(span.end)
    };
    segments.push(SimpleSegment {
        span,
        command_span: Span {
            start: command_start,
            end: command_end,
        },
    });
}

fn quoted_command_end(source: &str, start: usize, end: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    while index < end {
        if bytes[index] == b'^' && index + 1 < end {
            index += 2;
        } else if bytes[index] == b'"' {
            return index + 1;
        } else {
            index += 1;
        }
    }
    end
}

fn trimmed_span(source: &str, start: usize, end: usize) -> Option<Span> {
    let slice = &source[start..end];
    let leading = slice
        .char_indices()
        .find_map(|(index, character)| (!character.is_whitespace()).then_some(index))?;
    let trailing = slice.char_indices().rev().find_map(|(index, character)| {
        (!character.is_whitespace()).then_some(index + character.len_utf8())
    })?;
    Some(Span {
        start: start + leading,
        end: start + trailing,
    })
}

fn opaque_segment_reason(source: &str, segment: SimpleSegment) -> Option<OpaqueReason> {
    let command = &source[segment.command_span.start..segment.command_span.end];
    if command.is_empty() {
        return Some(OpaqueReason::MalformedInput);
    }
    let command = command
        .strip_prefix('"')
        .and_then(|command| command.strip_suffix('"'))
        .unwrap_or(command);
    let lower = command.to_ascii_lowercase();
    if matches!(lower.as_str(), "if" | "for" | "goto" | "call") {
        return Some(OpaqueReason::ControlCommand);
    }
    if lower.ends_with(".bat") || lower.ends_with(".cmd") {
        return Some(OpaqueReason::BatchInvocation);
    }
    if lower.len() == 2 && lower.as_bytes()[0].is_ascii_alphabetic() && lower.as_bytes()[1] == b':'
    {
        return Some(OpaqueReason::DriveChange);
    }
    None
}

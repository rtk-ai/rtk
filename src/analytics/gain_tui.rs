//! `rtk gain --watch`: a live, full-screen dashboard over the same two
//! sources the static `rtk gain` view reads -- RTK's own command-filtering
//! DB (`core::tracking::Tracker`) and the llm-proxy's usage DB (see
//! `crate::llm_proxy::read_gain_summary`). Refreshes on a timer; `q`/Esc/
//! Ctrl-C quits, `r` forces an immediate refresh.
//!
//! Opt-in only (`--watch`), so this never touches the plain-text default
//! view or the <10ms startup budget the rest of `rtk` is held to.

use crate::analytics::gain::share_cta_available;
use crate::core::tracking::{GainSummary as TrackerSummary, Tracker};
use crate::core::utils::{format_signed_tokens, format_tokens};
use crate::llm_proxy::{self, ProxyGainSummary};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table};
use std::time::{Duration, Instant};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(200);

struct State {
    tracker_summary: Option<TrackerSummary>,
    proxy_summary: Option<ProxyGainSummary>,
    last_refresh: Instant,
    project_scope: Option<String>,
}

impl State {
    fn refresh(&mut self) {
        self.tracker_summary = Tracker::new()
            .ok()
            .and_then(|t| t.get_summary_filtered(self.project_scope.as_deref()).ok());
        self.proxy_summary = llm_proxy::read_gain_summary().filter(|p| p.total_requests > 0);
        self.last_refresh = Instant::now();
    }
}

pub fn run(project_scope: Option<&str>) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut state = State {
        tracker_summary: None,
        proxy_summary: None,
        // Forces the very first draw to fetch data instead of showing "no
        // data yet" for one frame.
        last_refresh: Instant::now() - REFRESH_INTERVAL,
        project_scope: project_scope.map(str::to_string),
    };
    state.refresh();

    let result = event_loop(&mut terminal, &mut state);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, state: &mut State) -> Result<()> {
    loop {
        terminal.draw(|f| draw(f, state))?;

        if event::poll(POLL_INTERVAL)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('r') => state.refresh(),
                _ => {}
            }
        }

        if state.last_refresh.elapsed() >= REFRESH_INTERVAL {
            state.refresh();
        }
    }
}

fn draw(f: &mut Frame, state: &State) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(if share_cta_available() { 3 } else { 0 }),
            Constraint::Length(11),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, chunks[0], state);
    draw_kpi_panels(f, chunks[1], state);
    if share_cta_available() {
        draw_share_cta(f, chunks[2]);
    }
    draw_context_panel(
        f,
        chunks[3],
        state.tracker_summary.as_ref(),
        state.proxy_summary.as_ref(),
    );
    draw_table(f, chunks[4], state);
    draw_footer(f, chunks[5]);
}

/// A bordered panel right under the savings panels, in the same visual
/// language — the numbers above it are the thing worth sharing, so the
/// command to share them lives next to them.
fn draw_share_cta(f: &mut Frame, area: Rect) {
    let block = accent_block("Share it", Color::Green);
    let line = Line::from(vec![
        Span::styled("▶ share these numbers: ", Style::default().fg(Color::Green)),
        Span::styled(
            "rtk pipeline share",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   ·   publish them: ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            "rtk pipeline share publish",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    f.render_widget(Paragraph::new(line).block(block), area);
}

/// Accent colors, one per data source, reused across its panel's border,
/// title, and table so the eye can tell the two apart at a glance.
const TRACKER_ACCENT: Color = Color::Cyan;
const PROXY_ACCENT: Color = Color::LightMagenta;

/// Green/yellow/red tiering shared by every gauge and percentage figure, so
/// "this number is good/ok/bad" reads the same way everywhere on screen.
fn tier_color(pct: f64) -> Color {
    if pct >= 70.0 {
        Color::Green
    } else if pct >= 40.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn label_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn value_style() -> Style {
    Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

fn kpi_line(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), label_style()),
        Span::styled(value, value_style()),
    ])
}

fn kpi_line_tiered(label: &str, value: String, pct: f64) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), label_style()),
        Span::styled(
            value,
            Style::default()
                .fg(tier_color(pct))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

fn draw_header(f: &mut Frame, area: Rect, state: &State) {
    let title = Line::from(vec![
        Span::styled(
            " rtk gain ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Black)
                .bg(Color::Green),
        ),
        Span::styled(" live ", Style::default().fg(Color::Green)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .title(title);
    let ago = state.last_refresh.elapsed().as_secs();
    let ago_style = if ago == 0 {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Gray)
    };
    let line = Line::from(vec![
        Span::styled(format!("refreshed {ago}s ago"), ago_style),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "r",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" refresh now  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("/", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "Esc",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" quit", Style::default().fg(Color::DarkGray)),
    ]);
    let paragraph = Paragraph::new(line).block(block);
    f.render_widget(paragraph, area);
}

fn draw_kpi_panels(f: &mut Frame, area: Rect, state: &State) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    draw_tracker_panel(f, cols[0], state.tracker_summary.as_ref());
    draw_proxy_panel(f, cols[1], state.proxy_summary.as_ref());
}

fn kpi_layout(area: Rect) -> (Rect, Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Length(1)])
        .split(area);
    (rows[0], rows[1])
}

fn accent_block(title: &str, accent: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
}

fn draw_tracker_panel(f: &mut Frame, area: Rect, summary: Option<&TrackerSummary>) {
    let block = accent_block("Command filtering (rtk git/npm/cargo/...)", TRACKER_ACCENT);
    let Some(s) = summary else {
        f.render_widget(
            Paragraph::new(Span::styled("no data yet", label_style())).block(block),
            area,
        );
        return;
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (text_area, gauge_area) = kpi_layout(inner);

    let lines = vec![
        kpi_line("Commands", s.total_commands.to_string()),
        kpi_line("Input", format_tokens(s.total_input)),
        kpi_line("Output", format_tokens(s.total_output)),
        kpi_line_tiered(
            "Saved",
            format!(
                "{} ({:.1}%)",
                format_signed_tokens(s.total_saved),
                s.avg_savings_pct
            ),
            s.avg_savings_pct,
        ),
    ];
    let mut lines = lines;
    if s.oversized_excluded > 0 {
        lines.push(kpi_line(
            "Excluded",
            format!("{} oversized outputs", s.oversized_excluded),
        ));
    }
    f.render_widget(Paragraph::new(lines), text_area);
    f.render_widget(savings_gauge(s.avg_savings_pct), gauge_area);
}

fn draw_proxy_panel(f: &mut Frame, area: Rect, summary: Option<&ProxyGainSummary>) {
    // Spelled from the running executable, so a binary installed under another
    // name is not told to run a command it does not have. See `llm_proxy::cli_name`.
    let cli = crate::llm_proxy::cli_name();
    let block = accent_block(&format!("LLM Proxy ({cli} llm-proxy)"), PROXY_ACCENT);
    let Some(p) = summary else {
        f.render_widget(
            Paragraph::new(Span::styled(
                crate::llm_proxy::no_data_hint(),
                label_style(),
            ))
            .block(block),
            area,
        );
        return;
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (text_area, gauge_area) = kpi_layout(inner);

    let lines = vec![
        kpi_line("Requests", p.total_requests.to_string()),
        kpi_line(
            "Tokens",
            format!(
                "{} in / {} out",
                format_tokens(p.tokens_in.max(0) as usize),
                format_tokens(p.tokens_out.max(0) as usize)
            ),
        ),
        // Absolutes only: there is no honest stored denominator for a trim
        // percentage (prefix-dominated input; history re-counted by the
        // composition sums). Per-mechanism ratios live in the gain ledger.
        if p.trimmed_requests > 0 {
            kpi_line(
                "Trimmed",
                format!(
                    "{} across {} req touched",
                    format_tokens(p.tokens_saved.max(0) as usize),
                    p.trimmed_requests
                ),
            )
        } else {
            kpi_line(
                "Trimmed",
                format!(
                    "{} (transforms only)",
                    format_tokens(p.tokens_saved.max(0) as usize)
                ),
            )
        },
    ];
    let mut lines = lines;
    // Routing moves tokens to a cheaper biller instead of removing them, so
    // the trim figure above cannot see it. Tokens are the measured fact;
    // dollars appear only when the operator configured prices.
    if p.routed_requests > 0 {
        let mut line = format!(
            "{} req · {} in on the cheaper task model",
            p.routed_requests,
            crate::core::utils::format_tokens(p.routed_tokens_in.max(0) as usize),
        );
        if p.routed_cost_usd > 0.0 || p.routed_saved_usd > 0.0 {
            line.push_str(&format!(
                " · ${:.4} spent · {} saved est.",
                p.routed_cost_usd.max(0.0),
                crate::llm_proxy::approx_usd(p.routed_saved_usd)
            ));
        }
        lines.push(kpi_line("Routed", line));
    }
    if p.retrieve_calls > 0 {
        lines.push(kpi_line("Recovery", format!("{} calls", p.retrieve_calls)));
    }
    if let Some(hit) = p.cache_hit_ratio() {
        // A compressed body that is not byte-stable across turns invalidates
        // the upstream prefix, which costs more than the compression saves.
        // Surfaced next to Saved so the two are read together.
        let health = if p.cache_fractures > 0 {
            format!(
                "{:.1}%  ⚠ {} recent fractures",
                hit * 100.0,
                p.cache_fractures
            )
        } else {
            format!("{:.1}%", hit * 100.0)
        };
        lines.push(kpi_line("Cache hit", health));
        // The cache is usually the session's largest saver; the bare hit
        // ratio hides that. One user-agnostic percentage, est.-labeled.
        if let Some(effect) = p.cache_effect_pct() {
            lines.push(kpi_line(
                "Cache effect",
                format!("input billed at ~{:.0}% of raw est.", 100.0 - effect),
            ));
        }
    }
    f.render_widget(Paragraph::new(lines), text_area);
    // The gauge shows the panel's real lever: the share of input the cache
    // kept off the bill. A trim-over-all-input percentage would sit near zero
    // whenever caching works, punishing exactly the healthy state.
    let cache_saved_pct = p.cache_effect_pct().unwrap_or(0.0);
    f.render_widget(savings_gauge(cache_saved_pct), gauge_area);
}

/// One category's accent color, reused for its bar and its share label.
/// "Removed" bars (tiers of filtering, upstream to downstream) get their own
/// palette; "sent" bars (what's actually in the context) reuse the original
/// per-content-type palette.
const SAVED_RTK: Color = Color::Green;
const SAVED_INGEST_FILTER: Color = Color::LightGreen;
const SAVED_PIXEL_COMPRESS: Color = Color::LightCyan;
const SAVED_CCR: Color = Color::LightBlue;
const CTX_TOOL_RESULT: Color = Color::Yellow;
const CTX_TOOL_USE: Color = Color::Blue;
const CTX_IMAGE: Color = Color::Magenta;
const CTX_OTHER: Color = Color::Gray;

/// A single "label  ██████░░░░  NN%  (N tokens)" row.
fn composition_bar(
    label: &str,
    tokens: i64,
    total: i64,
    color: Color,
    width: usize,
) -> Line<'static> {
    let pct = if total > 0 {
        tokens as f64 / total as f64 * 100.0
    } else {
        0.0
    };
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    Line::from(vec![
        // Fixed width covers the longest label used ("RTK (bash/tool)", 15
        // chars) with a one-space gap -- `{:<N}` only pads when the label is
        // SHORTER than N, so anything longer breaks every bar's alignment.
        Span::styled(format!("{label:<16}"), label_style()),
        Span::styled(bar, Style::default().fg(color)),
        Span::styled(format!(" {pct:>4.1}%  "), Style::default().fg(color)),
        Span::styled(format_signed_tokens(tokens), label_style()),
    ])
}

/// Everything that would have filled the context window, combined across
/// BOTH savings sources: RTK's own command-output filtering (upstream --
/// bash/tool output trimmed before it ever reaches the proxy) and the LLM
/// proxy's own compression transforms (downstream -- what's left after RTK
/// is trimmed further, or not, before the model sees it). The "sent" bars
/// (what actually reached the model) are the proxy's `context_analysis`
/// breakdown; the "removed" bars are every filtering layer's own savings
/// counter, each already tracked independently, just never shown together.
/// `combined_total` is the sum of all eight, so every bar's percentage
/// reads as "share of everything that would have been in your context".
fn draw_context_panel(
    f: &mut Frame,
    area: Rect,
    tracker: Option<&TrackerSummary>,
    proxy: Option<&ProxyGainSummary>,
) {
    let block = accent_block("Context composition (RTK + LLM Proxy)", PROXY_ACCENT);
    let (Some(t), Some(p)) = (tracker, proxy) else {
        f.render_widget(
            Paragraph::new(Span::styled("no data yet", label_style())).block(block),
            area,
        );
        return;
    };
    if t.total_saved == 0 && p.ctx_total == 0 {
        f.render_widget(
            Paragraph::new(Span::styled("no data yet", label_style())).block(block),
            area,
        );
        return;
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rtk_saved = t.total_saved;
    let other = (p.ctx_total - p.ctx_tool_result - p.ctx_tool_use - p.ctx_image).max(0);
    let combined_total =
        rtk_saved + p.saved_ingest_filter + p.saved_pixel_compress + p.saved_ccr + p.ctx_total;
    let bar_width = 18;
    let bar =
        |label, tokens, color| composition_bar(label, tokens, combined_total, color, bar_width);

    let mut lines = vec![bar("RTK (bash/tool)", rtk_saved, SAVED_RTK)];
    // One bar per proxy-side method that actually removed something. The OSS
    // daemon ships CCR only, so `ingest`/`pixel` would otherwise be two
    // permanently-empty bars; they appear on their own the moment a build
    // with those stages writes the columns.
    for (method, tokens, color) in [
        ("ccr", p.saved_ccr, SAVED_CCR),
        ("ingest", p.saved_ingest_filter, SAVED_INGEST_FILTER),
        ("pixel", p.saved_pixel_compress, SAVED_PIXEL_COMPRESS),
    ] {
        if tokens > 0 {
            // Called directly rather than through the `bar` closure: the
            // label is a temporary, and the closure's inferred lifetime
            // would tie every call to it.
            lines.push(composition_bar(
                &format!("proxy: {method}"),
                tokens,
                combined_total,
                color,
                bar_width,
            ));
        }
    }
    lines.extend([
        bar("Tool output", p.ctx_tool_result, CTX_TOOL_RESULT),
        bar("Tool calls", p.ctx_tool_use, CTX_TOOL_USE),
        bar("Images", p.ctx_image, CTX_IMAGE),
        bar("Other/text", other, CTX_OTHER),
    ]);
    let dup_pct = if p.ctx_total > 0 {
        p.ctx_duplicate as f64 / p.ctx_total as f64 * 100.0
    } else {
        0.0
    };
    lines.push(Line::from(Span::styled(
        format!("{dup_pct:.1}% of what's sent is duplicate content repeated across blocks"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

fn savings_gauge(pct: f64) -> Gauge<'static> {
    let clamped = pct.clamp(0.0, 100.0);
    let color = tier_color(clamped);
    Gauge::default()
        .gauge_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .percent(clamped.round() as u16)
        .label(format!("{pct:.1}%"))
}

fn draw_table(f: &mut Frame, area: Rect, state: &State) {
    let block = accent_block("By command", TRACKER_ACCENT);
    let Some(s) = state.tracker_summary.as_ref() else {
        f.render_widget(
            Paragraph::new(Span::styled("no data yet", label_style())).block(block),
            area,
        );
        return;
    };
    if s.by_command.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("no data yet", label_style())).block(block),
            area,
        );
        return;
    }

    let header_style = Style::default()
        .fg(TRACKER_ACCENT)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let header = Row::new(vec![
        Cell::from("Command"),
        Cell::from("Count"),
        Cell::from("Saved"),
        Cell::from("Avg%"),
    ])
    .style(header_style);

    let rows: Vec<Row> = s
        .by_command
        .iter()
        .take(10)
        .enumerate()
        .map(|(idx, (cmd, count, saved, pct, _avg_time))| {
            let row_style = if idx % 2 == 1 {
                Style::default().add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(Span::styled(
                    cmd.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                )),
                Cell::from(count.to_string()),
                Cell::from(format_signed_tokens(*saved)),
                Cell::from(Span::styled(
                    format!("{pct:.1}%"),
                    Style::default().fg(tier_color(*pct)),
                )),
            ])
            .style(row_style)
        })
        .collect();

    let widths = [
        Constraint::Percentage(50),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
        Constraint::Percentage(15),
    ];
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let line = Line::from(Span::styled(
        " live view — data unaffected by watching it ",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));
    f.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn composition_bar_preserves_negative_savings() {
        let line = composition_bar("RTK (bash/tool)", -150, 200, SAVED_RTK, 18);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("-75.0%"), "{text}");
        assert!(text.ends_with("-150"), "{text}");
    }

    #[test]
    fn savings_gauge_keeps_signed_label_while_clamping_fill() {
        let mut terminal = Terminal::new(TestBackend::new(40, 1)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(savings_gauge(-75.0), frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("-75.0%"), "{text}");
    }

    #[test]
    fn tracker_panel_preserves_negative_savings() {
        let summary = TrackerSummary {
            total_commands: 2,
            total_input: 200,
            total_output: 350,
            total_saved: -150,
            avg_savings_pct: -75.0,
            total_time_ms: 0,
            avg_time_ms: 0,
            by_command: vec![],
            by_day: vec![],
            oversized_excluded: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| draw_tracker_panel(frame, frame.area(), Some(&summary)))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("-150 (-75.0%)"), "{text}");
    }
}

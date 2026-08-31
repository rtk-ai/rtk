//! Interactive local dashboard for RTK savings and integration health.

use crate::core::config::Config;
use crate::core::display_helpers::format_duration;
use crate::core::tracking::{current_project_path_string, GainSummary, Tracker};
use crate::core::utils::format_tokens;
use crate::hooks::hook_check::{self, HookStatus};
use anyhow::{Context, Result};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Tabs},
    Frame, Terminal,
};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TAB_NAMES: [&str; 5] = ["Overview", "Commands", "Activity", "Health", "Artifacts"];
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub fn run(project: bool) -> Result<()> {
    if !io::stdout().is_terminal() {
        return render_plain(project);
    }

    terminal::enable_raw_mode().context("Failed to enable dashboard input")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen, cursor::Hide) {
        let _ = terminal::disable_raw_mode();
        return Err(error).context("Failed to enter dashboard screen");
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
            return Err(error).context("Failed to initialize dashboard terminal");
        }
    };

    let result = dashboard_loop(&mut terminal, project);
    let disable_result =
        terminal::disable_raw_mode().context("Failed to disable dashboard raw mode");
    let leave_result = execute!(terminal.backend_mut(), cursor::Show, LeaveAlternateScreen)
        .context("Failed to leave dashboard screen");

    result.and(disable_result).and(leave_result)
}

fn dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    project: bool,
) -> Result<()> {
    let mut tab = 0usize;
    let mut data = DashboardData::load(project)?;
    let mut last_refresh = Instant::now();
    let mut status: Option<String> = None;
    let mut dirty = true;

    loop {
        if dirty {
            terminal
                .draw(|frame| draw(frame, tab, &data, status.as_deref()))
                .context("Failed to draw dashboard")?;
            dirty = false;
        }

        if event::poll(EVENT_POLL_INTERVAL).context("Failed to poll dashboard input")? {
            match event::read().context("Failed to read dashboard input")? {
                Event::Resize(_, _) => dirty = true,
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('1'..='5') => {
                            if let KeyCode::Char(value) = key.code {
                                let next = (value as usize) - ('1' as usize);
                                if next != tab {
                                    tab = next;
                                    dirty = true;
                                }
                            }
                        }
                        KeyCode::Tab | KeyCode::Right | KeyCode::Char('n') => {
                            tab = (tab + 1) % TAB_NAMES.len();
                            dirty = true;
                        }
                        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('p') => {
                            tab = (tab + TAB_NAMES.len() - 1) % TAB_NAMES.len();
                            dirty = true;
                        }
                        KeyCode::Char('r') => {
                            refresh_data(&mut data, project, &mut status);
                            last_refresh = Instant::now();
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if refresh_due(last_refresh.elapsed()) {
            refresh_data(&mut data, project, &mut status);
            last_refresh = Instant::now();
            dirty = true;
        }
    }
    Ok(())
}

fn refresh_due(elapsed: Duration) -> bool {
    elapsed >= AUTO_REFRESH_INTERVAL
}

fn refresh_data(data: &mut DashboardData, project: bool, status: &mut Option<String>) {
    match DashboardData::load(project) {
        Ok(refreshed) => {
            *data = refreshed;
            *status = None;
        }
        Err(error) => {
            *status = Some(format!("Refresh failed: {error:#}"));
        }
    }
}

fn draw(frame: &mut Frame, tab: usize, data: &DashboardData, status: Option<&str>) {
    // Every frame starts from blank cells. Ratatui diffs this complete buffer
    // against the previous one, so shorter tabs erase stale content without
    // issuing a visible terminal-wide clear.
    frame.render_widget(Clear, frame.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_tabs(frame, tab, data.project_path.is_some(), chunks[0]);
    match tab {
        0 => draw_overview(frame, data, chunks[1]),
        1 => draw_commands(frame, data, chunks[1]),
        2 => draw_activity(frame, data, chunks[1]),
        3 => draw_health(frame, data, chunks[1]),
        4 => draw_artifacts(frame, data, chunks[1]),
        _ => {}
    }
    draw_status(frame, status, chunks[2]);
}

fn draw_tabs(frame: &mut Frame, tab: usize, project: bool, area: Rect) {
    let scope = if project { "project" } else { "global" };
    let tabs = Tabs::new(TAB_NAMES)
        .block(Block::default().borders(Borders::ALL).title(format!(
            " RTK Dashboard v{} [{scope}] ",
            env!("CARGO_PKG_VERSION")
        )))
        .select(tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" | "));
    frame.render_widget(tabs, area);
}

fn draw_status(frame: &mut Frame, status: Option<&str>, area: Rect) {
    let (text, color) = match status {
        Some(message) => (format!(" {message}"), Color::Red),
        None => (
            format!(
                " q/Esc: quit | Tab/1-5 or n/p: tabs | r: refresh | auto-refresh: {}s",
                AUTO_REFRESH_INTERVAL.as_secs()
            ),
            Color::DarkGray,
        ),
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color).bg(Color::Black)),
        area,
    );
}

struct SummaryDisplay {
    commands: String,
    input: String,
    output: String,
    saved: String,
    savings: String,
    total_time: String,
    average_time: String,
}

fn summary_display(summary: &GainSummary) -> SummaryDisplay {
    SummaryDisplay {
        commands: summary.total_commands.to_string(),
        input: format_tokens(summary.total_input),
        output: format_tokens(summary.total_output),
        saved: format_tokens(summary.total_saved),
        savings: format!("{:.1}%", summary.avg_savings_pct),
        total_time: format_duration(summary.total_time_ms),
        average_time: format_duration(summary.avg_time_ms),
    }
}

struct CommandDisplay<'a> {
    command: &'a str,
    count: String,
    saved: String,
    savings: String,
    time: String,
}

fn command_display(
    command: &str,
    count: usize,
    saved: usize,
    savings: f64,
    time: u64,
) -> CommandDisplay<'_> {
    CommandDisplay {
        command,
        count: count.to_string(),
        saved: format_tokens(saved),
        savings: format!("{savings:.1}%"),
        time: format_duration(time),
    }
}

fn draw_overview(frame: &mut Frame, data: &DashboardData, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(0)])
        .split(area);
    let summary = &data.summary;
    let display = summary_display(summary);
    let bar_width = chunks[0].width.saturating_sub(26).min(60) as usize;
    let stats = vec![
        Line::from(vec![
            metric_span("Commands: ", display.commands),
            Span::raw("    "),
            metric_span("Input: ", display.input),
            Span::raw("    "),
            metric_span("Output: ", display.output),
        ]),
        Line::from(vec![
            metric_span("Tokens saved: ", display.saved),
            Span::raw(format!(" ({})", display.savings)),
        ]),
        Line::from(vec![
            metric_span("Execution time: ", display.total_time),
            Span::raw(" total    "),
            metric_span("Average: ", display.average_time),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Efficiency: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("[{}]", bar(summary.avg_savings_pct, bar_width)),
                Style::default().fg(Color::Green),
            ),
            Span::raw(format!(" {:.1}%", summary.avg_savings_pct)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(stats).block(panel("Savings Overview")),
        chunks[0],
    );
    draw_command_table(frame, summary, chunks[1], "Highest Impact Commands");
}

fn draw_commands(frame: &mut Frame, data: &DashboardData, area: Rect) {
    draw_command_table(
        frame,
        &data.summary,
        area,
        &format!(
            "Highest Impact Commands ({})",
            data.summary.by_command.len()
        ),
    );
}

fn draw_command_table(frame: &mut Frame, summary: &GainSummary, area: Rect, title: &str) {
    if summary.by_command.is_empty() {
        frame.render_widget(
            Paragraph::new("  No tracking data yet.").block(panel(title)),
            area,
        );
        return;
    }

    let capacity = table_row_capacity(area);
    let max_saved = summary
        .by_command
        .iter()
        .map(|(_, _, saved, _, _)| *saved)
        .max()
        .unwrap_or(1);
    let impact_width = area.width.saturating_sub(78).clamp(8, 28) as usize;
    let rows = summary.by_command.iter().take(capacity).enumerate().map(
        |(index, (command, count, saved, savings, time))| {
            let impact = ((*saved as f64 / max_saved as f64) * impact_width as f64)
                .round()
                .max(1.0) as usize;
            let display = command_display(command, *count, *saved, *savings, *time);
            Row::new(vec![
                Cell::from(format!("{}.", index + 1)),
                Cell::from(display.command).style(Style::default().fg(Color::Cyan)),
                Cell::from(display.count),
                Cell::from(display.saved),
                Cell::from(display.savings).style(Style::default().fg(if *savings >= 50.0 {
                    Color::Green
                } else {
                    Color::Red
                })),
                Cell::from(display.time),
                Cell::from("█".repeat(impact)).style(Style::default().fg(Color::Blue)),
            ])
        },
    );
    let header = Row::new(["#", "Command", "Count", "Saved", "Avg%", "Time", "Impact"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let widths = [
        Constraint::Length(4),
        Constraint::Min(18),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(impact_width as u16),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(1)
        .block(panel(title));
    frame.render_widget(table, area);
}

fn draw_activity(frame: &mut Frame, data: &DashboardData, area: Rect) {
    let summary = &data.summary;
    if summary.by_day.is_empty() {
        frame.render_widget(
            Paragraph::new("  No tracking data yet.").block(panel("Daily Activity")),
            area,
        );
        return;
    }

    let capacity = table_row_capacity(area);
    let shown = capacity.min(summary.by_day.len());
    let max_saved = summary
        .by_day
        .iter()
        .map(|(_, saved)| *saved)
        .max()
        .unwrap_or(1);
    let bar_width = area.width.saturating_sub(29).max(1) as usize;
    let rows = summary
        .by_day
        .iter()
        .rev()
        .take(capacity)
        .map(|(date, saved)| {
            let count = ((*saved as f64 / max_saved as f64) * bar_width as f64)
                .round()
                .max(1.0) as usize;
            Row::new(vec![
                Cell::from(date.as_str()),
                Cell::from(format_tokens(*saved)),
                Cell::from("█".repeat(count)).style(Style::default().fg(Color::Blue)),
            ])
        });
    let header = Row::new(["Date", "Saved", "Activity"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Min(1),
        ],
    )
    .header(header)
    .column_spacing(1)
    .block(panel(format!(
        "Daily Activity (showing {shown} of {} days)",
        summary.by_day.len()
    )));
    frame.render_widget(table, area);
}

fn draw_health(frame: &mut Frame, data: &DashboardData, area: Rect) {
    let (hook_text, hook_color) = match data.hook_status {
        HookStatus::Ok => ("ok", Color::Green),
        HookStatus::Outdated => ("outdated", Color::Yellow),
        HookStatus::Missing => ("missing", Color::Red),
    };
    let lines = vec![
        Line::from(""),
        health_line("Hook/plugin status: ", hook_text, hook_color),
        health_line(
            "Integration detected: ",
            if data.integration_installed {
                "yes"
            } else {
                "no"
            },
            if data.integration_installed {
                Color::Green
            } else {
                Color::Red
            },
        ),
        health_line(
            "Tracking scope: ",
            data.project_path.as_deref().unwrap_or("all projects"),
            Color::White,
        ),
        health_line(
            "Tee artifacts: ",
            &data.artifacts.len().to_string(),
            Color::White,
        ),
        health_line("MCP execution: ", "local stdio / bounded", Color::Green),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(panel("Integration Health")),
        area,
    );
}

fn draw_artifacts(frame: &mut Frame, data: &DashboardData, area: Rect) {
    if data.artifacts.is_empty() {
        frame.render_widget(
            Paragraph::new("  No tee artifacts found.").block(panel("Recent Tee Artifacts")),
            area,
        );
        return;
    }

    let capacity = table_row_capacity(area);
    let shown = capacity.min(data.artifacts.len());
    let rows = data.artifacts.iter().take(capacity).map(|artifact| {
        Row::new(vec![
            Cell::from(format_bytes(artifact.size)),
            Cell::from(artifact.path.to_string_lossy().into_owned())
                .style(Style::default().fg(Color::Cyan)),
        ])
    });
    let header = Row::new(["Size", "Path"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);
    let table = Table::new(rows, [Constraint::Length(10), Constraint::Min(1)])
        .header(header)
        .column_spacing(1)
        .block(panel(format!(
            "Recent Tee Artifacts (showing {shown} of {})",
            data.artifacts.len()
        )));
    frame.render_widget(table, area);
}

fn table_row_capacity(area: Rect) -> usize {
    // Two border rows plus a one-row header and its one-row bottom margin.
    area.height.saturating_sub(4) as usize
}

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {} ", title.into()))
        .title_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
}

fn metric_span(label: &'static str, value: String) -> Span<'static> {
    Span::styled(
        format!("  {label}{value}"),
        Style::default().fg(Color::Cyan),
    )
}

fn health_line(label: &'static str, value: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label}"), Style::default().fg(Color::Cyan)),
        Span::styled(value.to_string(), Style::default().fg(color)),
    ])
}

fn render_plain(project: bool) -> Result<()> {
    let data = DashboardData::load(project)?;
    let mut stdout = io::stdout();
    write!(stdout, "{}", plain_dashboard_text(&data, project))?;
    Ok(())
}

fn plain_dashboard_text(data: &DashboardData, project: bool) -> String {
    let display = summary_display(&data.summary);
    let mut lines = vec![
        format!(
            "RTK Dashboard v{} [{}]",
            env!("CARGO_PKG_VERSION"),
            if project { "project" } else { "global" }
        ),
        String::new(),
        format!("Commands:      {}", display.commands),
        format!("Input tokens:  {}", display.input),
        format!("Output tokens: {}", display.output),
        format!("Tokens saved:  {} ({})", display.saved, display.savings),
        format!(
            "Execution:     {} total / {} average",
            display.total_time, display.average_time
        ),
        String::new(),
        "Highest impact commands:".to_string(),
        format!(
            "  {:<36} {:>6} {:>9} {:>7} {:>8}",
            "Command", "Count", "Saved", "Avg%", "Time"
        ),
    ];
    lines.extend(data.summary.by_command.iter().take(10).map(
        |(command, count, saved, savings, time)| {
            let display = command_display(command, *count, *saved, *savings, *time);
            format!(
                "  {:<36} {:>6} {:>9} {:>7} {:>8}",
                display.command, display.count, display.saved, display.savings, display.time
            )
        },
    ));
    format!("{}\n", lines.join("\n"))
}

struct DashboardData {
    summary: GainSummary,
    artifacts: Vec<Artifact>,
    project_path: Option<String>,
    hook_status: HookStatus,
    integration_installed: bool,
}

impl DashboardData {
    fn load(project: bool) -> Result<Self> {
        let tracker = Tracker::new().context("Failed to initialize tracking database")?;
        let project_path = project.then(current_project_path_string);
        let summary = tracker
            .get_summary_filtered(project_path.as_deref())
            .context("Failed to load dashboard statistics")?;
        Ok(Self {
            summary,
            artifacts: list_artifacts()?,
            project_path,
            hook_status: hook_check::status(),
            integration_installed: hook_check::any_integration_installed(),
        })
    }
}

fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width.saturating_sub(filled))
    )
}

struct Artifact {
    path: PathBuf,
    size: u64,
}

fn list_artifacts() -> Result<Vec<Artifact>> {
    let config = Config::load().context("Failed to load RTK configuration")?;
    let Some(directory) = crate::core::tee::get_tee_dir(&config) else {
        return Ok(Vec::new());
    };
    let mut artifacts = std::fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "log") {
                return None;
            }
            let size = entry.metadata().ok()?.len();
            Some(Artifact { path, size })
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| right.path.cmp(&left.path));
    Ok(artifacts)
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn fixture() -> DashboardData {
        DashboardData {
            summary: GainSummary {
                total_commands: 2,
                total_input: 100,
                total_output: 20,
                total_saved: 80,
                avg_savings_pct: 80.0,
                total_time_ms: 50,
                avg_time_ms: 25,
                by_command: vec![
                    ("rtk git status".to_string(), 1, 80, 80.0, 25),
                    ("rtk rg TODO".to_string(), 1, 20, 20.0, 25),
                ],
                by_day: vec![
                    ("2026-07-27".to_string(), 20),
                    ("2026-07-28".to_string(), 80),
                ],
            },
            artifacts: Vec::new(),
            project_path: None,
            hook_status: HookStatus::Ok,
            integration_installed: true,
        }
    }

    fn backend_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut output = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                output.push_str(buffer[(x, y)].symbol());
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn auto_refresh_runs_on_icm_cadence() {
        assert!(!refresh_due(Duration::from_secs(29)));
        assert!(refresh_due(Duration::from_secs(30)));
        assert!(refresh_due(Duration::from_secs(31)));
    }

    #[test]
    fn status_footer_uses_the_configured_refresh_interval() {
        let backend = TestBackend::new(110, 1);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw_status(frame, None, frame.area()))
            .expect("status frame");
        let rendered = backend_text(&terminal);
        assert!(rendered.contains(&format!(
            "auto-refresh: {}s",
            AUTO_REFRESH_INTERVAL.as_secs()
        )));
    }

    #[test]
    fn plain_output_uses_shared_summary_and_command_fields() {
        let data = fixture();
        let summary = summary_display(&data.summary);
        let command = command_display("rtk git status", 1, 80, 80.0, 25);
        let rendered = plain_dashboard_text(&data, false);

        assert!(rendered.contains(&format!("Input tokens:  {}", summary.input)));
        assert!(rendered.contains(&format!("Output tokens: {}", summary.output)));
        assert!(rendered.contains(&format!(
            "Tokens saved:  {} ({})",
            summary.saved, summary.savings
        )));
        for heading in ["Command", "Count", "Saved", "Avg%", "Time"] {
            assert!(
                rendered.contains(heading),
                "missing command heading: {heading}"
            );
        }
        for field in [
            command.command,
            command.count.as_str(),
            command.saved.as_str(),
            command.savings.as_str(),
            command.time.as_str(),
        ] {
            assert!(rendered.contains(field), "missing shared field: {field}");
        }
    }

    #[test]
    fn visible_history_follows_terminal_height() {
        assert_eq!(table_row_capacity(Rect::new(0, 0, 100, 8)), 4);
        assert_eq!(table_row_capacity(Rect::new(0, 0, 100, 24)), 20);
        assert_eq!(table_row_capacity(Rect::new(0, 0, 100, 3)), 0);
    }

    #[test]
    fn framed_tab_switch_replaces_the_previous_screen() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let data = fixture();

        terminal
            .draw(|frame| draw(frame, 0, &data, None))
            .expect("overview frame");
        let overview = backend_text(&terminal);
        assert!(overview.contains("Savings Overview"));
        assert!(overview.contains('┌'));

        terminal
            .draw(|frame| draw(frame, 2, &data, None))
            .expect("activity frame");
        let activity = backend_text(&terminal);
        assert!(activity.contains("Daily Activity"));
        assert!(!activity.contains("Savings Overview"));
        assert!(!activity.contains("Highest Impact Commands"));
    }

    #[test]
    fn commands_screen_keeps_gain_columns() {
        let backend = TestBackend::new(110, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, 1, &fixture(), None))
            .expect("commands frame");
        let rendered = backend_text(&terminal);
        for heading in ["Command", "Count", "Saved", "Avg%", "Time", "Impact"] {
            assert!(rendered.contains(heading), "{heading}");
        }
        assert!(rendered.contains("rtk git status"));
    }
}

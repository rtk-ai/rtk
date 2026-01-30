//! Generic table display helpers for period-based statistics
//!
//! Eliminates duplication in gain.rs and cc_economics.rs by providing
//! a unified trait-based system for displaying daily/weekly/monthly data.

use crate::tracking::{DayStats, MonthStats, WeekStats};
use crate::utils::format_tokens;

/// Trait for period-based statistics that can be displayed in tables
pub trait PeriodStats {
    /// Icon for this period type (e.g., "📅", "📊", "📆")
    fn icon() -> &'static str;

    /// Label for this period type (e.g., "Daily", "Weekly", "Monthly")
    fn label() -> &'static str;

    /// Period identifier (e.g., "2026-01-20", "01-20 → 01-26", "2026-01")
    fn period(&self) -> String;

    /// Number of commands in this period
    fn commands(&self) -> usize;

    /// Input tokens in this period
    fn input_tokens(&self) -> usize;

    /// Output tokens in this period
    fn output_tokens(&self) -> usize;

    /// Saved tokens in this period
    fn saved_tokens(&self) -> usize;

    /// Savings percentage
    fn savings_pct(&self) -> f64;

    /// Period column width for alignment
    fn period_width() -> usize;

    /// Total separator line width
    fn separator_width() -> usize;
}

/// Generic table printer for any period statistics
pub fn print_period_table<T: PeriodStats>(data: &[T]) {
    if data.is_empty() {
        println!("No {} data available.", T::label().to_lowercase());
        return;
    }

    let period_width = T::period_width();
    let separator = "═".repeat(T::separator_width());

    println!(
        "\n{} {} Breakdown ({} {}s)",
        T::icon(),
        T::label(),
        data.len(),
        T::label().to_lowercase()
    );
    println!("{}", separator);
    println!(
        "{:<width$} {:>7} {:>10} {:>10} {:>10} {:>7}",
        match T::label() {
            "Weekly" => "Week",
            "Monthly" => "Month",
            _ => "Date",
        },
        "Cmds",
        "Input",
        "Output",
        "Saved",
        "Save%",
        width = period_width
    );
    println!("{}", "─".repeat(T::separator_width()));

    for period in data {
        println!(
            "{:<width$} {:>7} {:>10} {:>10} {:>10} {:>6.1}%",
            period.period(),
            period.commands(),
            format_tokens(period.input_tokens()),
            format_tokens(period.output_tokens()),
            format_tokens(period.saved_tokens()),
            period.savings_pct(),
            width = period_width
        );
    }

    // Compute totals
    let total_cmds: usize = data.iter().map(|d| d.commands()).sum();
    let total_input: usize = data.iter().map(|d| d.input_tokens()).sum();
    let total_output: usize = data.iter().map(|d| d.output_tokens()).sum();
    let total_saved: usize = data.iter().map(|d| d.saved_tokens()).sum();
    let avg_pct = if total_input > 0 {
        (total_saved as f64 / total_input as f64) * 100.0
    } else {
        0.0
    };

    println!("{}", "─".repeat(T::separator_width()));
    println!(
        "{:<width$} {:>7} {:>10} {:>10} {:>10} {:>6.1}%",
        "TOTAL",
        total_cmds,
        format_tokens(total_input),
        format_tokens(total_output),
        format_tokens(total_saved),
        avg_pct,
        width = period_width
    );
    println!();
}

// ── Trait Implementations ──

impl PeriodStats for DayStats {
    fn icon() -> &'static str {
        "📅"
    }

    fn label() -> &'static str {
        "Daily"
    }

    fn period(&self) -> String {
        self.date.clone()
    }

    fn commands(&self) -> usize {
        self.commands
    }

    fn input_tokens(&self) -> usize {
        self.input_tokens
    }

    fn output_tokens(&self) -> usize {
        self.output_tokens
    }

    fn saved_tokens(&self) -> usize {
        self.saved_tokens
    }

    fn savings_pct(&self) -> f64 {
        self.savings_pct
    }

    fn period_width() -> usize {
        12
    }

    fn separator_width() -> usize {
        64
    }
}

impl PeriodStats for WeekStats {
    fn icon() -> &'static str {
        "📊"
    }

    fn label() -> &'static str {
        "Weekly"
    }

    fn period(&self) -> String {
        let start = if self.week_start.len() > 5 {
            &self.week_start[5..]
        } else {
            &self.week_start
        };
        let end = if self.week_end.len() > 5 {
            &self.week_end[5..]
        } else {
            &self.week_end
        };
        format!("{} → {}", start, end)
    }

    fn commands(&self) -> usize {
        self.commands
    }

    fn input_tokens(&self) -> usize {
        self.input_tokens
    }

    fn output_tokens(&self) -> usize {
        self.output_tokens
    }

    fn saved_tokens(&self) -> usize {
        self.saved_tokens
    }

    fn savings_pct(&self) -> f64 {
        self.savings_pct
    }

    fn period_width() -> usize {
        22
    }

    fn separator_width() -> usize {
        72
    }
}

impl PeriodStats for MonthStats {
    fn icon() -> &'static str {
        "📆"
    }

    fn label() -> &'static str {
        "Monthly"
    }

    fn period(&self) -> String {
        self.month.clone()
    }

    fn commands(&self) -> usize {
        self.commands
    }

    fn input_tokens(&self) -> usize {
        self.input_tokens
    }

    fn output_tokens(&self) -> usize {
        self.output_tokens
    }

    fn saved_tokens(&self) -> usize {
        self.saved_tokens
    }

    fn savings_pct(&self) -> f64 {
        self.savings_pct
    }

    fn period_width() -> usize {
        10
    }

    fn separator_width() -> usize {
        64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_day_stats_trait() {
        let day = DayStats {
            date: "2026-01-20".to_string(),
            commands: 10,
            input_tokens: 1000,
            output_tokens: 500,
            saved_tokens: 200,
            savings_pct: 20.0,
        };

        assert_eq!(day.period(), "2026-01-20");
        assert_eq!(day.commands(), 10);
        assert_eq!(day.saved_tokens(), 200);
        assert_eq!(DayStats::icon(), "📅");
        assert_eq!(DayStats::label(), "Daily");
    }

    #[test]
    fn test_week_stats_trait() {
        let week = WeekStats {
            week_start: "2026-01-20".to_string(),
            week_end: "2026-01-26".to_string(),
            commands: 50,
            input_tokens: 5000,
            output_tokens: 2500,
            saved_tokens: 1000,
            savings_pct: 40.0,
        };

        assert_eq!(week.period(), "01-20 → 01-26");
        assert_eq!(WeekStats::icon(), "📊");
        assert_eq!(WeekStats::label(), "Weekly");
    }

    #[test]
    fn test_month_stats_trait() {
        let month = MonthStats {
            month: "2026-01".to_string(),
            commands: 200,
            input_tokens: 20000,
            output_tokens: 10000,
            saved_tokens: 5000,
            savings_pct: 50.0,
        };

        assert_eq!(month.period(), "2026-01");
        assert_eq!(MonthStats::icon(), "📆");
        assert_eq!(MonthStats::label(), "Monthly");
    }

    #[test]
    fn test_print_period_table_empty() {
        let data: Vec<DayStats> = vec![];
        print_period_table(&data);
        // Should print "No daily data available."
    }

    #[test]
    fn test_print_period_table_with_data() {
        let data = vec![
            DayStats {
                date: "2026-01-20".to_string(),
                commands: 10,
                input_tokens: 1000,
                output_tokens: 500,
                saved_tokens: 200,
                savings_pct: 20.0,
            },
            DayStats {
                date: "2026-01-21".to_string(),
                commands: 15,
                input_tokens: 1500,
                output_tokens: 750,
                saved_tokens: 300,
                savings_pct: 30.0,
            },
        ];
        print_period_table(&data);
        // Should print table with 2 rows + total
    }
}

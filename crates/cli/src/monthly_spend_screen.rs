//! The credit card spend trend screen (spec §13.4, D39) — ratatui's
//! first real chart widget in this codebase. No unit-test mandate for
//! rendering/interaction (§5) — the pure logic it calls into
//! (`monthly_spend.rs`) is what's actually unit-tested; this module is
//! verified manually against the running TUI.
//!
//! Sync, not async — same reasoning as `recommendations_screen.rs`:
//! nothing here does anything beyond synchronous local file reads.

use std::io;
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};
use ratatui::{Frame, Terminal};

use obol_core::{
    band_for_spend, extract_spend_series, MonthlySpendThresholds, SpendPoint, ThresholdBand,
    HISTORY_LIMIT,
};

const BLUE: Color = Color::Rgb(0, 114, 178);
const BLUISH_GREEN: Color = Color::Rgb(0, 158, 115);
const VERMILLION: Color = Color::Rgb(213, 94, 0);
const ORANGE: Color = Color::Rgb(230, 159, 0);

/// What the user asked for on their way out of this screen.
pub enum MonthlySpendAction {
    Quit,
    /// `v` was pressed — same key `recommendations_screen.rs` already
    /// uses for "back to Dashboard."
    GoToDashboard,
}

pub fn run(rules_path: &Path, snapshots_dir: &Path) -> io::Result<MonthlySpendAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let thresholds =
        obol_core::load_or_init_monthly_spend_thresholds(rules_path).unwrap_or_default();
    let snapshots = obol_core::load_recent_snapshots(snapshots_dir, HISTORY_LIMIT)
        .unwrap_or_default();
    let series = extract_spend_series(&snapshots);

    terminal.draw(|frame| draw(frame, &series, &thresholds))?;
    let action = loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('v') => break MonthlySpendAction::GoToDashboard,
            _ => break MonthlySpendAction::Quit,
        }
    };

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(action)
}

fn draw(frame: &mut Frame, series: &[SpendPoint], thresholds: &MonthlySpendThresholds) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(3), // status line
            Constraint::Min(10),   // chart
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Credit Card Spend Trend")
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        areas[0],
    );

    draw_status_line(frame, areas[1], series, thresholds);
    draw_chart(frame, areas[2], series, thresholds);

    frame.render_widget(Paragraph::new("v: view dashboard   (any other key): quit"), areas[3]);
}

/// Most recent point's total + band, matching the Dashboard summary's
/// figure exactly — computed the same way, `calculate_current_period_spend`
/// isn't reused here since this screen only has the historical series,
/// not a live `&[AccountRecord]`; the most recent `SpendPoint` in the
/// series stands in for it.
fn draw_status_line(
    frame: &mut Frame,
    area: Rect,
    series: &[SpendPoint],
    thresholds: &MonthlySpendThresholds,
) {
    let line = match series.last() {
        Some(point) => {
            let band = band_for_spend(point.total, thresholds);
            Line::from(vec![
                Span::raw("Most recent period: "),
                Span::styled(
                    format!("${:.2}", point.total),
                    Style::default()
                        .fg(threshold_band_color(band))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    band.label(),
                    Style::default()
                        .fg(threshold_band_color(band))
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        }
        None => Line::from(Span::raw("No snapshot history yet — run obol a few times first.")),
    };

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn threshold_band_color(band: ThresholdBand) -> Color {
    match band {
        ThresholdBand::Red => VERMILLION,
        ThresholdBand::Yellow => ORANGE,
        ThresholdBand::Green => BLUISH_GREEN,
    }
}

fn draw_chart(frame: &mut Frame, area: Rect, series: &[SpendPoint], thresholds: &MonthlySpendThresholds) {
    let points: Vec<(f64, f64)> = series
        .iter()
        .filter_map(|point| {
            point
                .timestamp
                .map(|ts| (ts.unix_timestamp() as f64, point.total))
        })
        .collect();

    if points.is_empty() {
        frame.render_widget(
            Paragraph::new("Not enough dated snapshot history to chart yet.")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    }

    let (x_min, x_max) = points.iter().fold(
        (f64::MAX, f64::MIN),
        |(min, max), (x, _)| (min.min(*x), max.max(*x)),
    );
    // A single point would otherwise collapse the x-axis to zero width
    // — pad by a day on each side so it still renders.
    const ONE_DAY_SECS: f64 = 86_400.0;
    let (x_min, x_max) = if x_min == x_max {
        (x_min - ONE_DAY_SECS, x_max + ONE_DAY_SECS)
    } else {
        (x_min, x_max)
    };

    let data_max = points.iter().map(|(_, y)| *y).fold(0.0_f64, f64::max);
    let y_max = data_max.max(thresholds.red_at_or_above) * 1.1;

    let yellow_line = [(x_min, thresholds.yellow_at_or_above), (x_max, thresholds.yellow_at_or_above)];
    let red_line = [(x_min, thresholds.red_at_or_above), (x_max, thresholds.red_at_or_above)];

    let datasets = vec![
        Dataset::default()
            .name("Spend")
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(BLUE))
            .data(&points),
        Dataset::default()
            .name("Yellow")
            .marker(symbols::Marker::Dot)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(ORANGE))
            .data(&yellow_line),
        Dataset::default()
            .name("Red")
            .marker(symbols::Marker::Dot)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(VERMILLION))
            .data(&red_line),
    ];

    // First/middle/last only — a terminal can't legibly show one label
    // per point across up to HISTORY_LIMIT snapshots.
    let x_labels = vec![
        Span::raw(format_unix_timestamp(x_min)),
        Span::raw(format_unix_timestamp((x_min + x_max) / 2.0)),
        Span::raw(format_unix_timestamp(x_max)),
    ];

    let chart = Chart::new(datasets)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Credit Card Liability Total Over Time"),
        )
        .x_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([x_min, x_max])
                .labels(x_labels),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().fg(Color::Gray))
                .bounds([0.0, y_max])
                .labels(vec![
                    Span::raw("$0"),
                    Span::raw(format!("${y_max:.0}")),
                ]),
        );

    frame.render_widget(chart, area);
}

fn format_unix_timestamp(secs: f64) -> String {
    let Ok(ts) = time::OffsetDateTime::from_unix_timestamp(secs as i64) else {
        return String::new();
    };
    format!("{:02}/{:02}", u8::from(ts.month()), ts.day())
}

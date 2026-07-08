//! The net worth dashboard screen (spec §13). No unit-test mandate for
//! rendering (§5 scopes test-first to core library logic) — verified
//! manually against the running TUI. Colors follow the Okabe–Ito
//! palette (§13) and are always paired with a text label, never the
//! only signal for status.

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;
use ratatui::Terminal;

use obol_core::{AccountRecord, Category, NetWorth, Snapshot, Status};

// Okabe–Ito palette (§13) — chosen over red/green specifically so
// status reads correctly under every common form of color vision.
const BLUE: Color = Color::Rgb(0, 114, 178);
const BLUISH_GREEN: Color = Color::Rgb(0, 158, 115);
const VERMILLION: Color = Color::Rgb(213, 94, 0);

/// What the user asked for on their way out of the Dashboard screen.
pub enum DashboardAction {
    Quit,
    /// `s` was pressed — jump to the Sources screen without exiting the
    /// process. `main.rs` loops between the two screens on this signal
    /// rather than requiring a full quit-and-rerun to manage sources.
    GoToSources,
}

/// Enters the alternate screen, draws the dashboard once (§13: "No
/// in-TUI refresh in v0.1" — getting new data means re-entering this
/// screen, not a live redraw loop within it), waits for a keypress,
/// then restores the terminal. Terminal setup/teardown is the one part
/// of this module that can't be unit-tested — a real terminal is being
/// taken over.
///
/// `previous` is the most recently saved snapshot *before* this run
/// (captured by the caller ahead of calling `run_and_save`, so it's
/// unambiguous regardless of whether this run's own save succeeds) —
/// used only to show "+$X since <date>" under the net worth figure.
/// `None` on a first-ever run, or if the previous snapshot's net worth
/// wasn't computable either (nothing meaningful to diff against).
pub fn run(snapshot: &Snapshot, previous: Option<&Snapshot>) -> io::Result<DashboardAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    terminal.draw(|frame| draw(frame, snapshot, previous))?;
    let action = loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('s') => break DashboardAction::GoToSources,
            _ => break DashboardAction::Quit,
        }
    };

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(action)
}

fn draw(frame: &mut Frame, snapshot: &Snapshot, previous: Option<&Snapshot>) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(4), // net worth + change-since-last-run
            Constraint::Min(3),    // assets
            Constraint::Min(3),    // liabilities
            Constraint::Length(1), // footer
        ])
        .split(frame.area());

    draw_title(frame, areas[0]);
    draw_net_worth(frame, areas[1], snapshot, previous);
    draw_group(
        frame,
        areas[2],
        "Assets",
        &snapshot.accounts,
        Category::Asset,
    );
    draw_group(
        frame,
        areas[3],
        "Liabilities",
        &snapshot.accounts,
        Category::Liability,
    );
    draw_footer(frame, areas[4]);
}

fn draw_title(frame: &mut Frame, area: Rect) {
    let title = Paragraph::new("obol — Financial Health Dashboard")
        .style(Style::default().add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, area);
}

fn draw_net_worth(frame: &mut Frame, area: Rect, snapshot: &Snapshot, previous: Option<&Snapshot>) {
    let current_net_worth = obol_core::calculate_net_worth_from_records(&snapshot.accounts);

    let mut lines = vec![match current_net_worth {
        NetWorth::Computed(total) => Line::from(vec![
            Span::raw("Net worth: "),
            Span::styled(
                format!("${total:.2}"),
                Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
            ),
        ]),
        // Never a numeric $0 — an explicit, differently-colored state
        // (§9.1, §13) so it can never be misread as a real zero balance.
        NetWorth::Unavailable { total_sources } => Line::from(Span::styled(
            format!("Net worth unavailable — 0/{total_sources} sources returned data this run"),
            Style::default().fg(VERMILLION).add_modifier(Modifier::BOLD),
        )),
    }];

    if let Some(change) = change_since_previous(current_net_worth, previous) {
        lines.push(change);
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// "+$X.XX since <date>" (or "-$X.XX"), comparing today's net worth to
/// the previous saved snapshot's — a deliberately simple stand-in for
/// spec §13's stretch-goal trend chart, not the chart itself. `None`
/// whenever a meaningful diff isn't possible: no previous snapshot yet
/// (first-ever run), or either snapshot's net worth wasn't computable
/// (§9.1 — diffing against/from an "unavailable" figure would be
/// misleading, not just unhelpful).
fn change_since_previous(current: NetWorth, previous: Option<&Snapshot>) -> Option<Line<'static>> {
    let NetWorth::Computed(current_total) = current else {
        return None;
    };
    let previous = previous?;
    let NetWorth::Computed(previous_total) =
        obol_core::calculate_net_worth_from_records(&previous.accounts)
    else {
        return None;
    };

    let delta = current_total - previous_total;
    let color = if delta > 0.0 {
        BLUISH_GREEN
    } else if delta < 0.0 {
        VERMILLION
    } else {
        BLUE
    };
    let sign = if delta >= 0.0 { "+" } else { "-" };
    let date = previous
        .created_at
        .split('T')
        .next()
        .unwrap_or(&previous.created_at);

    Some(Line::from(Span::styled(
        format!("{sign}${:.2} since {date}", delta.abs()),
        Style::default().fg(color),
    )))
}

fn draw_group(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    accounts: &[AccountRecord],
    category: Category,
) {
    let items: Vec<ListItem> = accounts
        .iter()
        .filter(|record| record.category() == category)
        .map(record_to_list_item)
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("(none configured)")])
    } else {
        List::new(items)
    };

    frame.render_widget(
        list.block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn record_to_list_item(record: &AccountRecord) -> ListItem<'static> {
    let name = format!("{} ({})", record.institution(), record.account_type());
    match record.status() {
        Status::Ok => {
            let balance = record
                .balance()
                .map(|b| format!("${b:.2}"))
                .unwrap_or_else(|| "—".to_string());
            ListItem::new(Line::from(vec![
                Span::raw(format!("{name}  ")),
                Span::styled(balance, Style::default().fg(BLUISH_GREEN)),
                Span::raw("  "),
                Span::styled("OK", Style::default().fg(BLUISH_GREEN)),
            ]))
        }
        // Every failed source shows a clear "unavailable" state with
        // its error message, never a blank panel (§13) — color plus the
        // "Failed" text label plus the message itself, never color alone.
        Status::Error | Status::Unknown => {
            let message = record.error_message().unwrap_or("unavailable");
            ListItem::new(Line::from(vec![
                Span::raw(format!("{name}  ")),
                Span::styled(
                    "Failed",
                    Style::default().fg(VERMILLION).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" — {message}")),
            ]))
        }
    }
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new("s: manage sources   (any other key): quit"),
        area,
    );
}

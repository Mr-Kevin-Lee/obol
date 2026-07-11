//! The Recommendations screen (spec §13, §14, D37) — currently just the
//! Type D checklist (7 fixed items, tri-state status). No unit-test
//! mandate for rendering/interaction (§5) — the pure logic it calls
//! into (`checklist.rs`) is what's actually unit-tested; this module is
//! verified manually against the running TUI.
//!
//! Sync, not async — unlike `sources_screen.rs`, nothing here does
//! anything beyond synchronous local file I/O (no Plaid-style awaited
//! HTTP/polling), so there's no reason to pay the async-fn tax. Still
//! callable fine from `main.rs`'s async `run_screen_loop` without
//! `.await`.

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use obol_core::{status_for, ChecklistItemStatus, ChecklistStatuses, CHECKLIST_ITEMS};

const BLUISH_GREEN: Color = Color::Rgb(0, 158, 115);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// What the user asked for on their way out of the Recommendations
/// screen.
pub enum RecommendationsAction {
    Quit,
    /// `v` was pressed — jump to the Dashboard without exiting the
    /// process (same key `sources_screen.rs` already uses for this).
    GoToDashboard,
}

pub fn run(rules_path: &Path) -> io::Result<RecommendationsAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut cursor: usize = 0;
    let result = event_loop(&mut terminal, rules_path, &mut cursor);

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}

fn event_loop(
    terminal: &mut Term,
    rules_path: &Path,
    cursor: &mut usize,
) -> io::Result<RecommendationsAction> {
    loop {
        let statuses = obol_core::load_or_init_checklist_statuses(rules_path).unwrap_or_default();

        if !CHECKLIST_ITEMS.is_empty() {
            *cursor = (*cursor).min(CHECKLIST_ITEMS.len() - 1);
        }

        terminal.draw(|frame| draw_list(frame, &statuses, *cursor))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(RecommendationsAction::Quit),
            KeyCode::Char('v') => return Ok(RecommendationsAction::GoToDashboard),
            KeyCode::Down | KeyCode::Char('j') if !CHECKLIST_ITEMS.is_empty() => {
                *cursor = (*cursor + 1) % CHECKLIST_ITEMS.len();
            }
            KeyCode::Up | KeyCode::Char('k') if !CHECKLIST_ITEMS.is_empty() => {
                *cursor = (*cursor + CHECKLIST_ITEMS.len() - 1) % CHECKLIST_ITEMS.len();
            }
            KeyCode::Char(' ') => {
                if let Some(item) = CHECKLIST_ITEMS.get(*cursor) {
                    let current = status_for(&statuses, item.id);
                    let _ = obol_core::set_checklist_item_status(
                        rules_path,
                        item.id,
                        current.cycle(),
                    );
                }
            }
            _ => {}
        }
    }
}

fn draw_list(frame: &mut Frame, statuses: &ChecklistStatuses, cursor: usize) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(3),    // checklist
            Constraint::Length(1), // footer/help
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Recommendations — Checklist")
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        areas[0],
    );

    let items: Vec<ListItem> = CHECKLIST_ITEMS
        .iter()
        .enumerate()
        .map(|(i, item)| checklist_list_item(item, status_for(statuses, item.id), i == cursor))
        .collect();
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL)),
        areas[1],
    );

    frame.render_widget(
        Paragraph::new("space: cycle status   v: view dashboard   q: quit"),
        areas[2],
    );
}

fn checklist_list_item(
    item: &obol_core::ChecklistItem,
    status: ChecklistItemStatus,
    is_selected: bool,
) -> ListItem<'static> {
    let prefix = if is_selected { "> " } else { "  " };
    let (symbol, color) = match status {
        ChecklistItemStatus::Complete => ("[x]", Some(BLUISH_GREEN)),
        ChecklistItemStatus::Incomplete => ("[ ]", None),
        ChecklistItemStatus::NotApplicable => ("[-]", None),
    };

    let mut style = Style::default();
    if let Some(color) = color {
        style = style.fg(color);
    }

    ListItem::new(Line::from(vec![
        Span::raw(format!("{prefix}{symbol} ")),
        Span::styled(item.description.to_string(), style),
    ]))
}

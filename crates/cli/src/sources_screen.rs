//! The Sources screen (spec §10.1, §13): list configured sources with
//! their health, edit a source's category/type/institution, and remove
//! one. Accounts are added by `obol link simplefin`, not here. No
//! unit-test mandate for rendering/interaction (§5) — the validation
//! logic it calls into (`form.rs`) is what's actually unit-tested; this
//! module is verified manually against the running TUI.

use std::io::{self, Stdout};
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};

use obol_core::{Category, Snapshot, SourceConfig, Status};

use crate::form::{self, SourceFormInput};

fn category_label(category: Category) -> &'static str {
    match category {
        Category::Asset => "asset",
        Category::Liability => "liability",
        Category::Unknown => "unknown",
    }
}

const VERMILLION: Color = Color::Rgb(213, 94, 0);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// What the user asked for on their way out of the Sources screen.
pub enum SourcesAction {
    Quit,
    /// `v` was pressed — jump to the Dashboard without exiting the
    /// process (the counterpart to Dashboard's `s`, see dashboard.rs).
    GoToDashboard,
}

pub fn run(sources_path: &Path, snapshots_dir: &Path) -> io::Result<SourcesAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut selected: usize = 0;
    let result = event_loop(&mut terminal, sources_path, snapshots_dir, &mut selected);

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}

fn event_loop(
    terminal: &mut Term,
    sources_path: &Path,
    snapshots_dir: &Path,
    selected: &mut usize,
) -> io::Result<SourcesAction> {
    loop {
        let sources = obol_core::load_or_init(sources_path).unwrap_or_default();
        let recent = obol_core::load_recent_snapshots(snapshots_dir, 1).unwrap_or_default();

        if !sources.is_empty() {
            *selected = (*selected).min(sources.len() - 1);
        }

        terminal.draw(|frame| draw_list(frame, &sources, &recent, *selected))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(SourcesAction::Quit),
            KeyCode::Char('v') => return Ok(SourcesAction::GoToDashboard),
            KeyCode::Down | KeyCode::Char('j') if !sources.is_empty() => {
                *selected = (*selected + 1) % sources.len();
            }
            KeyCode::Up | KeyCode::Char('k') if !sources.is_empty() => {
                *selected = (*selected + sources.len() - 1) % sources.len();
            }
            KeyCode::Char('e') => {
                if let Some(source) = sources.get(*selected) {
                    edit_flow(terminal, sources_path, &sources, source)?;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('r') => {
                if let Some(source) = sources.get(*selected) {
                    remove_flow(terminal, sources_path, source)?;
                }
            }
            _ => {}
        }
    }
}

fn draw_list(frame: &mut Frame, sources: &[SourceConfig], recent: &[Snapshot], selected: usize) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Min(3),    // source list
            Constraint::Length(1), // footer/help
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Sources")
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        areas[0],
    );

    let items: Vec<ListItem> = if sources.is_empty() {
        vec![ListItem::new(
            "(no sources configured — quit and run `obol link simplefin` to add your accounts)",
        )]
    } else {
        sources
            .iter()
            .enumerate()
            .map(|(i, source)| source_list_item(source, recent, i == selected))
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL)),
        areas[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(
            "e: edit   d: remove   v: view dashboard   q: quit   (add accounts: obol link simplefin)",
        )),
        areas[2],
    );
}

fn source_list_item(
    source: &SourceConfig,
    recent: &[Snapshot],
    is_selected: bool,
) -> ListItem<'static> {
    let health = health_for(source, recent);
    let prefix = if is_selected { "> " } else { "  " };
    let text = format!(
        "{prefix}{} — {} / {} / {}  [{}]",
        source.id,
        source.provider,
        category_label(source.category),
        source.account_type,
        health
    );
    ListItem::new(text)
}

fn health_for(source: &SourceConfig, recent: &[Snapshot]) -> String {
    let Some(latest) = recent.first() else {
        return "No snapshot yet — press 'v' to fetch".to_string();
    };
    match latest.accounts.iter().find(|r| r.source_id() == source.id) {
        Some(record) if record.status() == Status::Ok => "OK".to_string(),
        Some(record) => format!(
            "Failed: {}",
            record.error_message().unwrap_or("unknown error")
        ),
        // Distinct from the no-snapshot-at-all case above: a snapshot
        // exists, but this particular source wasn't in it — e.g. it was
        // just linked since the last fetch.
        None => "New — not included in the last fetch yet, press 'v' to fetch".to_string(),
    }
}

fn edit_flow(
    terminal: &mut Term,
    sources_path: &Path,
    existing: &[SourceConfig],
    source: &SourceConfig,
) -> io::Result<()> {
    let initial = SourceFormInput {
        id: source.id.clone(),
        category: category_label(source.category).to_string(),
        account_type: source.account_type.clone(),
        institution: source.institution.clone(),
    };

    let existing_ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let Some(input) = gather_form_input(terminal, &initial, &existing_ids, &source.id)? else {
        return Ok(());
    };

    // Each field was already validated as it was entered
    // (gather_form_input) — this is a final defense-in-depth check right
    // before writing, not something a well-behaved single session should
    // ever actually hit.
    let errors = form::validate(&input, &existing_ids, &source.id);
    if !errors.is_empty() {
        return show_message(
            terminal,
            &format!("Could not save source:\n{}", errors.join("\n")),
        );
    }

    let config = form::to_source_config(&input, source);
    if let Err(err) = obol_core::edit_source(sources_path, &source.id, config) {
        return show_message(terminal, &format!("Failed to save source: {err}"));
    }
    Ok(())
}

fn remove_flow(terminal: &mut Term, sources_path: &Path, source: &SourceConfig) -> io::Result<()> {
    let Some(confirmed) = prompt_line(
        terminal,
        &format!("Remove '{}'? type 'yes' to confirm", source.id),
        "",
    )?
    else {
        return Ok(());
    };
    if confirmed != "yes" {
        return Ok(());
    }
    if let Err(err) = obol_core::remove_source(sources_path, &source.id) {
        return show_message(terminal, &format!("Failed to remove source: {err}"));
    }
    Ok(())
}

/// Category is picked from a list rather than typed — free-text entry for
/// a fixed set of choices just invites typos (`select_prompt`).
const CATEGORY_OPTIONS: &[(&str, &str)] = &[
    (
        "asset",
        "Asset — checking, savings, investment, retirement, etc.",
    ),
    ("liability", "Liability — credit card, loan, mortgage, etc."),
];

/// Sequentially prompts for every editable field, `Esc`-cancelable at any
/// point (returns `None` if the user backs out partway through). Each
/// field is validated as it's entered (`prompt_line_validated`) — an
/// invalid value re-prompts *that* field only, with what was typed still
/// there to fix, rather than discarding the whole form.
fn gather_form_input(
    terminal: &mut Term,
    initial: &SourceFormInput,
    existing_ids: &[String],
    editing_id: &str,
) -> io::Result<Option<SourceFormInput>> {
    let Some(id) = prompt_line_validated(
        terminal,
        "short internal name (e.g. chase_checking, no spaces)",
        &initial.id,
        |value| {
            if value.trim().is_empty() {
                return Err("must not be empty".to_string());
            }
            if editing_id != value && existing_ids.iter().any(|id| id == value) {
                return Err(format!("a source with id '{value}' already exists"));
            }
            Ok(())
        },
    )?
    else {
        return Ok(None);
    };

    let category_index = CATEGORY_OPTIONS
        .iter()
        .position(|(value, _)| *value == initial.category)
        .unwrap_or(0);
    let Some(category) = select_prompt(terminal, "category", CATEGORY_OPTIONS, category_index)?
    else {
        return Ok(None);
    };

    let type_label = if category == "liability" {
        "account type (e.g. credit_card, mortgage, student_loan)"
    } else {
        "account type (e.g. checking, savings, brokerage, retirement_401k)"
    };
    let Some(account_type) =
        prompt_line_validated(terminal, type_label, &initial.account_type, |value| {
            if value.trim().is_empty() {
                Err("must not be empty".to_string())
            } else {
                Ok(())
            }
        })?
    else {
        return Ok(None);
    };
    let Some(institution) = prompt_line_validated(
        terminal,
        "institution (e.g. Chase, Vanguard)",
        &initial.institution,
        |value| {
            if value.trim().is_empty() {
                Err("must not be empty".to_string())
            } else {
                Ok(())
            }
        },
    )?
    else {
        return Ok(None);
    };

    Ok(Some(SourceFormInput {
        id,
        category,
        account_type,
        institution,
    }))
}

/// Like `prompt_line`, but loops on an invalid value: shows what's
/// wrong, then re-prompts the *same* field with what was already typed
/// still in the buffer, rather than bubbling the error up and
/// discarding every field gathered so far.
fn prompt_line_validated(
    terminal: &mut Term,
    label: &str,
    initial: &str,
    validate: impl Fn(&str) -> Result<(), String>,
) -> io::Result<Option<String>> {
    let mut current = initial.to_string();
    loop {
        let Some(value) = prompt_line(terminal, label, &current)? else {
            return Ok(None);
        };
        match validate(&value) {
            Ok(()) => return Ok(Some(value)),
            Err(message) => {
                current = value;
                show_message(terminal, &message)?;
            }
        }
    }
}

/// Picks one of `options` (`(internal_value, display_label)` pairs) via
/// up/down + Enter, starting at `initial_index`. Returns the
/// `internal_value` of whichever option was selected — the user never
/// types or sees the internal string directly.
fn select_prompt(
    terminal: &mut Term,
    label: &str,
    options: &[(&str, &str)],
    initial_index: usize,
) -> io::Result<Option<String>> {
    let mut selected = initial_index.min(options.len().saturating_sub(1));
    loop {
        terminal.draw(|frame| {
            let area = prompt_area(frame.area(), 1 + options.len() as u16);
            let mut lines = vec![Line::from(format!("{label}:"))];
            lines.extend(options.iter().enumerate().map(|(i, (_, display))| {
                let prefix = if i == selected { "> " } else { "  " };
                Line::from(format!("{prefix}{display}"))
            }));
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
                area,
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(Some(options[selected].0.to_string())),
            KeyCode::Esc => return Ok(None),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % options.len(),
            KeyCode::Up | KeyCode::Char('k') => {
                selected = (selected + options.len() - 1) % options.len();
            }
            _ => {}
        }
    }
}

/// Single-line text prompt, pre-filled with `initial`. `Enter` confirms,
/// `Esc` cancels the whole form (not just this field) — a deliberate
/// simplification over per-field cancel, since a partially-filled source
/// isn't meaningfully save-able anyway.
fn prompt_line(terminal: &mut Term, label: &str, initial: &str) -> io::Result<Option<String>> {
    let mut buffer = initial.to_string();
    loop {
        terminal.draw(|frame| {
            let area = prompt_area(frame.area(), 1);
            let text = format!("{label}: {buffer}");
            frame.render_widget(
                Paragraph::new(text).block(Block::default().borders(Borders::ALL)),
                area,
            );
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(Some(buffer)),
            KeyCode::Esc => return Ok(None),
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Char(c) => buffer.push(c),
            _ => {}
        }
    }
}

fn show_message(terminal: &mut Term, message: &str) -> io::Result<()> {
    let lines = message.lines().count() as u16 + 2; // + blank line + "press any key"
    terminal.draw(|frame| {
        let area = prompt_area(frame.area(), lines);
        frame.render_widget(
            Paragraph::new(format!("{message}\n\n(press any key to continue)"))
                .style(Style::default().fg(VERMILLION))
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
    })?;
    event::read()?;
    Ok(())
}

fn prompt_area(area: Rect, content_lines: u16) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(content_lines + 2), Constraint::Min(0)])
        .split(area)[0]
}

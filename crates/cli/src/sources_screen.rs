//! The Sources screen (spec §10.1, §13): list configured sources with
//! health + the Plaid Item usage indicator, generic add/edit/remove
//! forms for the three non-Plaid providers (`manual_entry`,
//! `webdriver`, `statement_import`), and a real "Connect via Plaid"
//! Hosted Link flow (task 25). No unit-test mandate for
//! rendering/interaction (§5) — the validation logic it calls into
//! (`form.rs`) is what's actually unit-tested; this module is verified
//! manually against the running TUI.
//!
//! **The Plaid flow persists through the D24 dev-bridge, not real
//! Keychain storage** — `complete_plaid_link` (in core) falls back to
//! embedding the token in `sources.yaml` itself whenever the real
//! Keychain write fails, which it currently always does (the parked
//! signing bug). Once D24 is actually fixed, this same code path starts
//! using real Keychain storage automatically, no changes needed here.

use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

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
use ratatui::{Frame, Terminal};

use obol_core::{
    Category, ItemUsageCounter, LinkAccount, LinkSession, PlaidClient, PlaidConfig,
    PlaidEnvironment, SelectedAccount, Snapshot, SourceConfig, Status,
};
use secrecy::Secret;

use crate::form::{self, SourceFormInput};

fn category_label(category: Category) -> &'static str {
    match category {
        Category::Asset => "asset",
        Category::Liability => "liability",
        Category::Unknown => "unknown",
    }
}

const BLUISH_GREEN: Color = Color::Rgb(0, 158, 115);
const ORANGE: Color = Color::Rgb(230, 159, 0);
const VERMILLION: Color = Color::Rgb(213, 94, 0);

type Term = Terminal<CrosstermBackend<Stdout>>;

/// What the user asked for on their way out of the Sources screen.
pub enum SourcesAction {
    Quit,
    /// `v` was pressed — jump to the Dashboard without exiting the
    /// process (the counterpart to Dashboard's `s`, see dashboard.rs).
    GoToDashboard,
}

pub async fn run(
    sources_path: &Path,
    item_usage_path: &Path,
    snapshots_dir: &Path,
) -> io::Result<SourcesAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut selected: usize = 0;
    let result = event_loop(
        &mut terminal,
        sources_path,
        item_usage_path,
        snapshots_dir,
        &mut selected,
    )
    .await;

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    result
}

async fn event_loop(
    terminal: &mut Term,
    sources_path: &Path,
    item_usage_path: &Path,
    snapshots_dir: &Path,
    selected: &mut usize,
) -> io::Result<SourcesAction> {
    loop {
        let sources = obol_core::load_or_init(sources_path).unwrap_or_default();
        let item_usage = obol_core::load_or_init_item_usage(item_usage_path)
            .unwrap_or_else(|_| ItemUsageCounter::new());
        let recent = obol_core::load_recent_snapshots(snapshots_dir, 1).unwrap_or_default();

        if !sources.is_empty() {
            *selected = (*selected).min(sources.len() - 1);
        }

        terminal.draw(|frame| draw_list(frame, &sources, &item_usage, &recent, *selected))?;

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
            KeyCode::Char('a') => add_flow(terminal, sources_path, &sources)?,
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
            KeyCode::Char('p') => {
                plaid_connect_flow(terminal, sources_path, item_usage_path, &sources).await?;
            }
            _ => {}
        }
    }
}

fn draw_list(
    frame: &mut Frame,
    sources: &[SourceConfig],
    item_usage: &ItemUsageCounter,
    recent: &[Snapshot],
    selected: usize,
) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // title
            Constraint::Length(1), // item usage indicator
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

    frame.render_widget(Paragraph::new(item_usage_line(item_usage)), areas[1]);

    let items: Vec<ListItem> = if sources.is_empty() {
        vec![ListItem::new(
            "(no sources configured — press 'a' to add one)",
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
        areas[2],
    );

    frame.render_widget(
        Paragraph::new(
            "a: add   e: edit   d: remove   p: connect via Plaid   v: view dashboard   q: quit",
        ),
        areas[3],
    );
}

fn item_usage_line(counter: &ItemUsageCounter) -> Line<'static> {
    let text = format!(
        "Plaid Items: {}/{} used",
        counter.count(),
        obol_core::PLAID_ITEM_LIMIT
    );
    let color = if counter.is_blocked() {
        VERMILLION
    } else if counter.is_at_warning_threshold() {
        ORANGE
    } else {
        BLUISH_GREEN
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
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
        // just added (by hand or auto-discovered) since the last fetch.
        None => "New — not included in the last fetch yet, press 'v' to fetch".to_string(),
    }
}

fn add_flow(terminal: &mut Term, sources_path: &Path, existing: &[SourceConfig]) -> io::Result<()> {
    let existing_ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let Some(input) =
        gather_form_input(terminal, &SourceFormInput::default(), &existing_ids, None)?
    else {
        return Ok(());
    };

    // Each field was already validated as it was entered
    // (gather_form_input) — this is a final defense-in-depth check
    // right before writing, in case sources.yaml changed underneath us
    // (e.g. no cross-process lock is wired into the CLI yet), not
    // something a well-behaved single session should ever actually hit.
    let errors = form::validate(&input, &existing_ids, None);
    if !errors.is_empty() {
        return show_message(
            terminal,
            &format!("Could not add source:\n{}", errors.join("\n")),
        );
    }

    let config = form::to_source_config(&input);
    if let Err(err) = obol_core::add_source(sources_path, config) {
        return show_message(terminal, &format!("Failed to add source: {err}"));
    }
    Ok(())
}

fn edit_flow(
    terminal: &mut Term,
    sources_path: &Path,
    existing: &[SourceConfig],
    source: &SourceConfig,
) -> io::Result<()> {
    let initial = SourceFormInput {
        id: source.id.clone(),
        provider: source.provider.clone(),
        category: category_label(source.category).to_string(),
        account_type: source.account_type.clone(),
        institution: source.institution.clone(),
        webdriver_login_url: source
            .provider_config
            .get("login_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        watch_dir: source
            .provider_config
            .get("watch_dir")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        account_hint: source
            .provider_config
            .get("account_hint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    };

    let existing_ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let Some(input) = gather_form_input(terminal, &initial, &existing_ids, Some(&source.id))?
    else {
        return Ok(());
    };

    let errors = form::validate(&input, &existing_ids, Some(&source.id));
    if !errors.is_empty() {
        return show_message(
            terminal,
            &format!("Could not save source:\n{}", errors.join("\n")),
        );
    }

    let config = form::to_source_config(&input);
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

/// "Connect via Plaid" (spec §10.1's Hosted Link flow, task 25):
/// creates a Link token, opens it in the system browser, blocks the
/// screen while polling for completion (`Esc` cancels), then lets the
/// user pick which returned accounts to actually track (D23) before
/// writing them via `complete_plaid_link`. A blocking poll rather than
/// spec's D18 cancelable-background-task design — a deliberate v1
/// simplification given the screen's current synchronous model.
async fn plaid_connect_flow(
    terminal: &mut Term,
    sources_path: &Path,
    item_usage_path: &Path,
    existing: &[SourceConfig],
) -> io::Result<()> {
    let (Ok(client_id), Ok(secret)) = (
        std::env::var("PLAID_CLIENT_ID"),
        std::env::var("PLAID_SECRET"),
    ) else {
        return show_message(
            terminal,
            "PLAID_CLIENT_ID/PLAID_SECRET are not set — export them before running \
             obol to connect via Plaid.",
        );
    };
    let environment = if std::env::var("PLAID_ENVIRONMENT").as_deref() == Ok("production") {
        PlaidEnvironment::Production
    } else {
        PlaidEnvironment::Sandbox
    };
    let client = PlaidClient::new(PlaidConfig {
        client_id,
        secret: Secret::new(secret),
        environment,
    });

    let mut item_counter = obol_core::load_or_init_item_usage(item_usage_path)
        .unwrap_or_else(|_| ItemUsageCounter::new());
    if item_counter.is_blocked() {
        return show_message(
            terminal,
            "Plaid Item limit reached (10/10) — see §7.1 for alternatives \
             (manual entry, webdriver, or upgrading off the Trial plan).",
        );
    }

    let link = match client.create_link_token("obol-single-user", "Obol").await {
        Ok(link) => link,
        Err(err) => {
            return show_message(terminal, &format!("Could not start Plaid Link: {err}"));
        }
    };
    let Some(url) = link.hosted_link_url.clone() else {
        return show_message(
            terminal,
            "Plaid didn't return a hosted_link_url for this Link token — can't continue.",
        );
    };

    // Best-effort: if this fails (headless environment, `open` not on
    // PATH), the URL is still shown on the polling screen to open or
    // copy manually.
    let _ = std::process::Command::new("open").arg(&url).spawn();

    let Some(session) = poll_for_link_completion(terminal, &client, &link.link_token, &url).await?
    else {
        return Ok(()); // canceled
    };

    let Some(public_token) = session.public_token().map(str::to_string) else {
        return show_message(
            terminal,
            "The Plaid Link session finished without completing — it may have been \
             abandoned or hit an error. Try again from the Sources screen.",
        );
    };

    let Some(item_result) = session.results.item_add_results.first() else {
        return show_message(terminal, "Plaid didn't return any accounts for this Item.");
    };
    let institution_name = item_result.institution.name.clone();
    let accounts = item_result.accounts.clone();

    let Some(picked_indices) = multi_select_accounts(terminal, &accounts)? else {
        return Ok(()); // canceled
    };

    let mut existing_ids: Vec<String> = existing.iter().map(|s| s.id.clone()).collect();
    let mut selected_accounts = Vec::new();
    for &i in &picked_indices {
        let Some(selected) =
            gather_plaid_account_input(terminal, &accounts[i], &institution_name, &existing_ids)?
        else {
            return Ok(()); // canceled partway — nothing written yet
        };
        existing_ids.push(selected.source_id.clone());
        selected_accounts.push(selected);
    }

    let result = obol_core::complete_plaid_link(
        &client,
        &public_token,
        selected_accounts,
        &mut item_counter,
        sources_path,
    )
    .await;
    // The Item counter reflects whatever happened on Plaid's side
    // (incremented before sources.yaml writes even start, D23) — save
    // it regardless of whether the write loop below succeeded fully,
    // since the Item was really created either way.
    let _ = obol_core::save_item_usage(item_usage_path, &item_counter);

    match result {
        Ok(()) => show_message(terminal, "Connected successfully."),
        Err(err) => show_message(terminal, &format!("Could not finish connecting: {err}")),
    }
}

/// Blocks on a redraw + `Esc`-check tick (`TICK`) rather than sleeping
/// for the full poll interval, so canceling stays responsive; only
/// actually calls `get_link_token_status` once every `TICKS_PER_POLL`
/// ticks, matching the ~5s cadence `plaid_link_spike.rs` already uses.
async fn poll_for_link_completion(
    terminal: &mut Term,
    client: &PlaidClient,
    link_token: &str,
    url: &str,
) -> io::Result<Option<LinkSession>> {
    const TICK: Duration = Duration::from_millis(500);
    const TICKS_PER_POLL: u32 = 10;
    let mut ticks = 0u32;

    loop {
        terminal.draw(|frame| {
            let area = prompt_area(frame.area(), 4);
            let lines = vec![
                Line::from("Waiting for you to complete Plaid Link in your browser..."),
                Line::from(format!("URL: {url}")),
                Line::from(""),
                Line::from("Press Esc to cancel"),
            ];
            frame.render_widget(
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL)),
                area,
            );
        })?;

        if event::poll(TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Esc {
                    return Ok(None);
                }
            }
            continue;
        }

        ticks += 1;
        if ticks < TICKS_PER_POLL {
            continue;
        }
        ticks = 0;

        match client.get_link_token_status(link_token).await {
            Ok(status) => {
                if let Some(session) = status.link_sessions.into_iter().find(|s| s.is_finished()) {
                    return Ok(Some(session));
                }
            }
            Err(err) => {
                show_message(terminal, &format!("Error checking Link status: {err}"))?;
                return Ok(None);
            }
        }
    }
}

/// Checklist of every account this Item returned (space to toggle,
/// enter to confirm — requires at least one selected, esc cancels the
/// whole flow). Returns the selected indices into `accounts`.
fn multi_select_accounts(
    terminal: &mut Term,
    accounts: &[LinkAccount],
) -> io::Result<Option<Vec<usize>>> {
    let mut selected = vec![false; accounts.len()];
    let mut cursor = 0usize;
    loop {
        terminal.draw(|frame| {
            let area = prompt_area(frame.area(), 1 + accounts.len() as u16);
            let mut lines = vec![Line::from(
                "Select accounts to track (space to toggle, enter to confirm):",
            )];
            lines.extend(accounts.iter().enumerate().map(|(i, account)| {
                let checkbox = if selected[i] { "[x]" } else { "[ ]" };
                let cursor_marker = if i == cursor { "> " } else { "  " };
                let mask = account
                    .mask
                    .as_deref()
                    .map(|m| format!(" (...{m})"))
                    .unwrap_or_default();
                Line::from(format!("{cursor_marker}{checkbox} {}{mask}", account.name))
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
            KeyCode::Enter => {
                let picked: Vec<usize> = selected
                    .iter()
                    .enumerate()
                    .filter(|(_, &is_selected)| is_selected)
                    .map(|(i, _)| i)
                    .collect();
                if !picked.is_empty() {
                    return Ok(Some(picked));
                }
            }
            KeyCode::Esc => return Ok(None),
            KeyCode::Char(' ') => selected[cursor] = !selected[cursor],
            KeyCode::Down | KeyCode::Char('j') => cursor = (cursor + 1) % accounts.len(),
            KeyCode::Up | KeyCode::Char('k') => {
                cursor = (cursor + accounts.len() - 1) % accounts.len();
            }
            _ => {}
        }
    }
}

/// Gathers the local id/category/type for one selected Plaid account.
/// Category defaults from Plaid's own account `type` ("credit"/"loan"
/// → liability, else asset) and account type defaults from Plaid's
/// `subtype` — both editable, not forced.
fn gather_plaid_account_input(
    terminal: &mut Term,
    account: &LinkAccount,
    institution_name: &str,
    existing_ids: &[String],
) -> io::Result<Option<SelectedAccount>> {
    let suggested_id = suggest_source_id(institution_name, &account.name, existing_ids);
    let Some(source_id) = prompt_line_validated(
        terminal,
        &format!("short internal name for '{}'", account.name),
        &suggested_id,
        |value| {
            if value.trim().is_empty() {
                return Err("must not be empty".to_string());
            }
            if existing_ids.iter().any(|id| id == value) {
                return Err(format!("a source with id '{value}' already exists"));
            }
            Ok(())
        },
    )?
    else {
        return Ok(None);
    };

    let default_category_index =
        usize::from(matches!(account.account_type.as_str(), "credit" | "loan"));
    let Some(category_str) = select_prompt(
        terminal,
        "category",
        CATEGORY_OPTIONS,
        default_category_index,
    )?
    else {
        return Ok(None);
    };
    let category = match category_str.as_str() {
        "liability" => Category::Liability,
        _ => Category::Asset,
    };

    let default_type = account
        .subtype
        .clone()
        .unwrap_or_else(|| account.account_type.clone());
    let Some(account_type) = prompt_line_validated(
        terminal,
        "account type (e.g. checking, savings, credit_card)",
        &default_type,
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

    Ok(Some(SelectedAccount {
        source_id,
        plaid_account_id: account.id.clone(),
        category,
        account_type,
        institution: institution_name.to_string(),
    }))
}

/// A reasonable starting id, deduplicated against `existing_ids` — the
/// prompt is always editable, this just saves typing the common case.
fn suggest_source_id(
    institution_name: &str,
    account_name: &str,
    existing_ids: &[String],
) -> String {
    let slug = format!("{institution_name}_{account_name}")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>();

    let mut candidate = slug.clone();
    let mut suffix = 2;
    while existing_ids.iter().any(|id| id == &candidate) {
        candidate = format!("{slug}_{suffix}");
        suffix += 1;
    }
    candidate
}

/// Internal provider names aren't meaningful to a user, and free-text
/// entry for a fixed set of choices just invites typos — both provider
/// and category are picked from a list instead (`select_prompt`), not
/// typed.
const PROVIDER_OPTIONS: &[(&str, &str)] = &[
    (
        "manual_entry",
        "Manual entry — you type the balance in yourself each run",
    ),
    (
        "webdriver",
        "Browser automation — logs in to a bank website for you",
    ),
    (
        "statement_import",
        "Statement dropbox — reads the balance out of PDF statements you drop in a folder",
    ),
];

const CATEGORY_OPTIONS: &[(&str, &str)] = &[
    (
        "asset",
        "Asset — checking, savings, investment, retirement, etc.",
    ),
    ("liability", "Liability — credit card, loan, mortgage, etc."),
];

/// Sequentially prompts for every field a generic source needs,
/// `Esc`-cancelable at any point (returns `None` if the user backs out
/// partway through). Each field is validated as it's entered
/// (`prompt_line_validated`) — an invalid value re-prompts *that* field
/// only, with what was typed still there to fix, rather than discarding
/// the whole form. `webdriver`'s extra `login_url` field, and
/// `statement_import`'s `watch_dir`/`account_hint` fields, are only
/// prompted for once that provider has actually been picked —
/// `account_hint` is the one optional field in either group, so an
/// empty answer there means "no hint," not a validation error.
fn gather_form_input(
    terminal: &mut Term,
    initial: &SourceFormInput,
    existing_ids: &[String],
    editing_id: Option<&str>,
) -> io::Result<Option<SourceFormInput>> {
    let Some(id) = prompt_line_validated(
        terminal,
        "short internal name (e.g. chase_checking, no spaces)",
        &initial.id,
        |value| {
            if value.trim().is_empty() {
                return Err("must not be empty".to_string());
            }
            if editing_id != Some(value) && existing_ids.iter().any(|id| id == value) {
                return Err(format!("a source with id '{value}' already exists"));
            }
            Ok(())
        },
    )?
    else {
        return Ok(None);
    };

    let provider_index = PROVIDER_OPTIONS
        .iter()
        .position(|(value, _)| *value == initial.provider)
        .unwrap_or(0);
    let Some(provider) = select_prompt(terminal, "provider", PROVIDER_OPTIONS, provider_index)?
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
    let webdriver_login_url = if provider == "webdriver" {
        match prompt_line_validated(
            terminal,
            "login URL (the bank's login page, e.g. https://example.com/login)",
            initial.webdriver_login_url.as_deref().unwrap_or(""),
            |value| {
                if value.starts_with("http://") || value.starts_with("https://") {
                    Ok(())
                } else {
                    Err("must start with http:// or https://".to_string())
                }
            },
        )? {
            Some(url) => Some(url),
            None => return Ok(None),
        }
    } else {
        None
    };
    let (watch_dir, account_hint) = if provider == "statement_import" {
        let Some(watch_dir) = prompt_line_validated(
            terminal,
            "directory to watch for PDF statements (e.g. /Users/you/Statements/Chase)",
            initial.watch_dir.as_deref().unwrap_or(""),
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
        let Some(account_hint) = prompt_line(
            terminal,
            "account hint (optional — last-4 digits, or an employer/plan-name substring for \
             Fidelity NetBenefits; only needed if a statement covers more than one account)",
            initial.account_hint.as_deref().unwrap_or(""),
        )?
        else {
            return Ok(None);
        };
        let account_hint = (!account_hint.trim().is_empty()).then_some(account_hint);
        (Some(watch_dir), account_hint)
    } else {
        (None, None)
    };

    Ok(Some(SourceFormInput {
        id,
        provider,
        category,
        account_type,
        institution,
        webdriver_login_url,
        watch_dir,
        account_hint,
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
/// simplification over per-field cancel, since a partially-filled
/// generic source isn't meaningfully save-able anyway.
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

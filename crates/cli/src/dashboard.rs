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

use obol_core::{
    calculate_current_period_spend, completion_summary, status_for, AccountRecord, AssetClass,
    Category, ChecklistItemStatus, ChecklistStatuses, CurrentPeriodSpend, EmergencyFundStatus,
    EmergencyFundThresholds, Holding, MonthlySpendThresholds, NetWorth, Snapshot, Status,
    ThresholdBand, CHECKLIST_ITEMS,
};

// Okabe–Ito palette (§13) — chosen over red/green specifically so
// status reads correctly under every common form of color vision.
const BLUE: Color = Color::Rgb(0, 114, 178);
const BLUISH_GREEN: Color = Color::Rgb(0, 158, 115);
const VERMILLION: Color = Color::Rgb(213, 94, 0);
const ORANGE: Color = Color::Rgb(230, 159, 0);

/// What the user asked for on their way out of the Dashboard screen.
pub enum DashboardAction {
    Quit,
    /// `s` was pressed — jump to the Sources screen without exiting the
    /// process. `main.rs` loops between the two screens on this signal
    /// rather than requiring a full quit-and-rerun to manage sources.
    GoToSources,
    /// `r` was pressed — jump to the Recommendations screen (spec §13,
    /// D37) without exiting the process, same shape as `GoToSources`.
    GoToRecommendations,
    /// `c` was pressed — jump to the credit card spend trend chart
    /// screen (spec §13.4, D39), same shape as `GoToSources`.
    GoToMonthlySpendChart,
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
///
/// `emergency_fund_thresholds` (spec §13.1 Type A, D36) — loaded and,
/// if needed, interactively prompted for by the caller *before* this
/// function is entered, since that prompt needs plain cooked-terminal
/// stdin, not the raw/alternate-screen mode this function sets up.
///
/// `checklist_statuses` (spec §13.1 Type D, D37) — loaded fresh by the
/// caller on every entry, same treatment as `emergency_fund_thresholds`.
///
/// `monthly_spend_thresholds` (spec §13.4, D39) — same reload-per-entry
/// treatment.
pub fn run(
    snapshot: &Snapshot,
    previous: Option<&Snapshot>,
    emergency_fund_thresholds: &EmergencyFundThresholds,
    checklist_statuses: &ChecklistStatuses,
    monthly_spend_thresholds: &MonthlySpendThresholds,
) -> io::Result<DashboardAction> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    terminal.draw(|frame| {
        draw(
            frame,
            snapshot,
            previous,
            emergency_fund_thresholds,
            checklist_statuses,
            monthly_spend_thresholds,
        )
    })?;
    let action = loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('s') => break DashboardAction::GoToSources,
            KeyCode::Char('r') => break DashboardAction::GoToRecommendations,
            KeyCode::Char('c') => break DashboardAction::GoToMonthlySpendChart,
            _ => break DashboardAction::Quit,
        }
    };

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(action)
}

fn draw(
    frame: &mut Frame,
    snapshot: &Snapshot,
    previous: Option<&Snapshot>,
    emergency_fund_thresholds: &EmergencyFundThresholds,
    checklist_statuses: &ChecklistStatuses,
    monthly_spend_thresholds: &MonthlySpendThresholds,
) {
    // Spec D31: pooled across every holdings-bearing account in this
    // snapshot (today, at most one — Vanguard Brokerage), not just the
    // first one found. Absent entirely (not an empty panel) whenever no
    // account has any — most accounts/snapshots never will.
    let all_holdings: Vec<Holding> = snapshot
        .accounts
        .iter()
        .filter_map(|record| record.holdings())
        .flatten()
        .cloned()
        .collect();
    let buckets = obol_core::bucket(&all_holdings);

    let emergency_fund_status =
        obol_core::calculate_emergency_fund_status(&snapshot.accounts, emergency_fund_thresholds);
    let monthly_spend_status =
        calculate_current_period_spend(&snapshot.accounts, monthly_spend_thresholds);

    let mut constraints = vec![
        Constraint::Length(3), // title
        Constraint::Length(4), // net worth + change-since-last-run
        // Spec §13.1 Type A, D36: always rendered, unlike the
        // conditional holdings-breakdown panel below — an unconfigured/
        // no-data state is itself meaningful status worth always
        // surfacing (same "not buried in a details view" instinct as
        // the Plaid Item usage counter, §7.1), not just hidden.
        Constraint::Length(3), // emergency fund coverage
        // Spec §13.4, D39: same always-rendered instinct.
        Constraint::Length(3), // credit card spend trend summary
        // Spec §13.1 Type D, D37: same always-rendered instinct as
        // emergency fund coverage above — the full 7-item list, not
        // just a count, so the current state is visible without
        // leaving the Dashboard (+2 for the panel's own border/title
        // lines, one line per item, same shape as the holdings-
        // breakdown panel below).
        Constraint::Length(CHECKLIST_ITEMS.len() as u16 + 2), // checklist
    ];
    if !buckets.is_empty() {
        // +2 for the panel's own border/title lines, one line per bucket.
        constraints.push(Constraint::Length(buckets.len() as u16 + 2));
    }
    constraints.push(Constraint::Min(3)); // assets
    constraints.push(Constraint::Min(3)); // liabilities
    constraints.push(Constraint::Length(1)); // footer

    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    draw_title(frame, areas[0]);
    draw_net_worth(frame, areas[1], snapshot, previous);
    draw_emergency_fund_coverage(frame, areas[2], &emergency_fund_status);
    draw_monthly_spend_summary(frame, areas[3], &monthly_spend_status);
    draw_checklist(frame, areas[4], checklist_statuses);

    let mut next = 5;
    if !buckets.is_empty() {
        draw_holdings_breakdown(frame, areas[next], &buckets);
        next += 1;
    }
    draw_group(
        frame,
        areas[next],
        "Assets",
        &snapshot.accounts,
        Category::Asset,
    );
    next += 1;
    draw_group(
        frame,
        areas[next],
        "Liabilities",
        &snapshot.accounts,
        Category::Liability,
    );
    next += 1;
    draw_footer(frame, areas[next]);
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

/// Emergency fund coverage (spec §13.1 Type A, D36) — one line, always
/// rendered (see `draw`'s comment on why this differs from the
/// conditional holdings-breakdown panel below). Band color + text label
/// together, never color alone (FR27).
fn draw_emergency_fund_coverage(frame: &mut Frame, area: Rect, status: &EmergencyFundStatus) {
    let line = match status {
        EmergencyFundStatus::Computed {
            months_of_coverage,
            band,
            ..
        } => Line::from(vec![
            Span::raw("Emergency fund: "),
            Span::styled(
                format!("{months_of_coverage:.1} months of expenses covered"),
                Style::default()
                    .fg(threshold_band_color(*band))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                band.label(),
                Style::default()
                    .fg(threshold_band_color(*band))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        EmergencyFundStatus::NoCashAccountData => Line::from(Span::raw(
            "Emergency fund: no checking/money-market data this run",
        )),
        // Mostly a fallback for "the first-run prompt was skipped" —
        // main.rs prompts for this interactively before the dashboard
        // is ever entered, so this state is reachable but not the
        // common path.
        EmergencyFundStatus::TargetNotConfigured => Line::from(Span::raw(
            "Emergency fund: target monthly expenses not set — rerun to be prompted, or edit rules.yaml",
        )),
    };

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// Credit card spend trend summary (spec §13.4, D39) — one line, always
/// rendered, mirroring `draw_emergency_fund_coverage` exactly. Computed
/// on the live, just-fetched snapshot (not reloaded from disk), same as
/// the emergency-fund panel. The actual chart lives on its own screen
/// (`'c'`) — a chart needs real space to be legible, and the Dashboard
/// is already dense.
fn draw_monthly_spend_summary(frame: &mut Frame, area: Rect, status: &CurrentPeriodSpend) {
    let line = match status {
        CurrentPeriodSpend::Computed { total, band } => Line::from(vec![
            Span::raw("Credit card spend: "),
            Span::styled(
                format!("${total:.2} this period"),
                Style::default()
                    .fg(threshold_band_color(*band))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                band.label(),
                Style::default()
                    .fg(threshold_band_color(*band))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  (press 'c' for chart)"),
        ]),
        CurrentPeriodSpend::NoLiabilityAccountData => Line::from(Span::raw(
            "Credit card spend: no liability account data this run",
        )),
    };

    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// The full checklist (spec §13.1 Type D, D37) — every item and its
/// current status, always rendered (same "not buried in a details
/// view" instinct as the emergency-fund panel above), not just a
/// count. Read-only here; toggling happens on the Recommendations
/// screen (`'r'`). Mirrors `draw_holdings_breakdown`'s "one line per
/// entry inside a bordered panel" shape. `[x]`/`[ ]`/`[-]` symbol
/// markers disambiguate every state regardless of color (FR27);
/// `BLUISH_GREEN` is added on top for Complete only, matching
/// `recommendations_screen.rs`'s own rendering exactly.
fn draw_checklist(frame: &mut Frame, area: Rect, statuses: &ChecklistStatuses) {
    let (complete, total) = completion_summary(statuses);

    let lines: Vec<Line> = CHECKLIST_ITEMS
        .iter()
        .map(|item| {
            let status = status_for(statuses, item.id);
            let (symbol, color) = match status {
                ChecklistItemStatus::Complete => ("[x]", Some(BLUISH_GREEN)),
                ChecklistItemStatus::Incomplete => ("[ ]", None),
                ChecklistItemStatus::NotApplicable => ("[-]", None),
            };
            let mut style = Style::default();
            if let Some(color) = color {
                style = style.fg(color);
            }
            Line::from(vec![
                Span::raw(format!("{symbol} ")),
                Span::styled(item.description, style),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Checklist — {complete}/{total} complete (press 'r' to manage)")),
        ),
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

/// One horizontal bar per asset class (cash/ETF/individual stock, spec
/// D31) — the terminal-native stand-in for a pie chart agreed on for
/// this feature, since ratatui has no native pie/donut widget. `buckets`
/// is already-aggregated `(AssetClass, dollar total)` pairs from
/// `obol_core::bucket` — this function only renders, it never touches
/// raw holdings or classification itself.
fn draw_holdings_breakdown(frame: &mut Frame, area: Rect, buckets: &[(AssetClass, f64)]) {
    const BAR_WIDTH: usize = 30;
    let total: f64 = buckets.iter().map(|(_, value)| value).sum();

    let lines: Vec<Line> = buckets
        .iter()
        .map(|(class, value)| {
            let fraction = if total > 0.0 { value / total } else { 0.0 };
            let filled = ((fraction * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled));

            Line::from(vec![
                Span::raw(format!("{:<16}", class.label())),
                Span::styled(bar, Style::default().fg(asset_class_color(*class))),
                Span::raw(format!("  {:>5.1}%  ${value:.2}", fraction * 100.0)),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Holdings Breakdown"),
        ),
        area,
    );
}

fn asset_class_color(class: AssetClass) -> Color {
    match class {
        AssetClass::Cash => BLUE,
        AssetClass::Fund => BLUISH_GREEN,
        AssetClass::Stock => VERMILLION,
    }
}

fn draw_group(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    accounts: &[AccountRecord],
    category: Category,
) {
    let matching: Vec<&AccountRecord> = accounts
        .iter()
        .filter(|record| record.category() == category)
        .collect();

    let items: Vec<ListItem> = matching.iter().map(|record| record_to_list_item(record)).collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("(none configured)")])
    } else {
        List::new(items)
    };

    // Sum of only Status::Ok balances — an errored account has no
    // trustworthy figure to add in, same "excluded from the total"
    // instinct calculate_net_worth already applies (§12, §9.1), just
    // per-group rather than net.
    let subtotal: f64 = matching
        .iter()
        .filter(|record| record.status() == Status::Ok)
        .filter_map(|record| record.balance())
        .sum();
    let panel_title = format!("{title} — ${subtotal:.2}");

    frame.render_widget(
        list.block(Block::default().borders(Borders::ALL).title(panel_title)),
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
        Paragraph::new("s: manage sources   r: checklist   c: spend chart   (any other key): quit"),
        area,
    );
}

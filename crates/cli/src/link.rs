//! `obol link simplefin` (issue 37): claim a Setup Token, store the Access
//! URL in the Keychain, then create one source per Bridge account. Plain
//! stdin/stdout, run before the TUI starts. The decisions (which accounts
//! are new, what to guess, the source id) live in `obol_core::simplefin_link`.

use std::io::Write;
use std::path::Path;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use obol_core::Category;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Choice {
    Asset,
    Liability,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretKey {
    Pending,
    Done,
    Cancelled,
}

/// One key press of the hidden Setup Token prompt. Pure so the editing
/// rules are testable without a terminal.
fn apply_key(buf: &mut String, key: KeyEvent) -> SecretKey {
    if key.kind != KeyEventKind::Press {
        return SecretKey::Pending;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Enter => SecretKey::Done,
        KeyCode::Esc => SecretKey::Cancelled,
        KeyCode::Char('c') if ctrl => SecretKey::Cancelled,
        KeyCode::Backspace => {
            buf.pop();
            SecretKey::Pending
        }
        KeyCode::Char(c) if !ctrl => {
            buf.push(c);
            SecretKey::Pending
        }
        _ => SecretKey::Pending,
    }
}

/// `input` is one line the user typed; blank takes `default`.
fn parse_choice(input: &str, default: Choice) -> Option<Choice> {
    match input.trim().to_lowercase().as_str() {
        "" => Some(default),
        "a" | "asset" => Some(Choice::Asset),
        "l" | "liability" => Some(Choice::Liability),
        "s" | "skip" => Some(Choice::Skip),
        _ => None,
    }
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Reads a line without echoing it. `None` means the user cancelled.
fn read_hidden(prompt: &str) -> Result<Option<String>, String> {
    print!("{prompt}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    crossterm::terminal::enable_raw_mode()
        .map_err(|e| format!("could not read hidden input (is stdin a terminal?): {e}"))?;
    let guard = RawModeGuard;

    let mut buf = String::new();
    let outcome = loop {
        if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
            match apply_key(&mut buf, key) {
                SecretKey::Pending => {}
                SecretKey::Done => break Some(buf),
                SecretKey::Cancelled => break None,
            }
        }
    };
    drop(guard);
    println!();
    Ok(outcome)
}

/// `None` on end of input.
fn prompt_line(prompt: &str) -> Result<Option<String>, String> {
    print!("{prompt}");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut input = String::new();
    let read = std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;
    Ok((read > 0).then(|| input.trim().to_string()))
}

fn prompt_choice(default: Choice) -> Result<Choice, String> {
    let hint = match default {
        Choice::Liability => "[a]sset / [L]iability / [s]kip, default liability",
        _ => "[A]sset / [l]iability / [s]kip, default asset",
    };
    loop {
        let line = prompt_line(&format!("  {hint}: "))?.ok_or("input closed")?;
        match parse_choice(&line, default) {
            Some(choice) => return Ok(choice),
            None => println!("  please answer a, l or s"),
        }
    }
}

pub async fn run_simplefin(sources_path: &Path) {
    if let Err(message) = link_simplefin(sources_path).await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn link_simplefin(sources_path: &Path) -> Result<(), String> {
    let mut sources = obol_core::load_or_init(sources_path)
        .map_err(|e| format!("sources.yaml could not be loaded: {e}"))?;
    let client = obol_core::SimpleFinClient::default();

    let token = read_hidden(
        "SimpleFIN Setup Token (from https://bridge.simplefin.org/simplefin/create; \
         blank reuses the stored Access URL): ",
    )?
    .ok_or("cancelled")?;

    let access_url = if token.trim().is_empty() {
        obol_core::read_simplefin_access_url()
            .map_err(|_| "no stored Access URL yet — paste a Setup Token".to_string())?
    } else {
        // Claiming is one-time, so prove the Keychain accepts a write first.
        obol_core::probe_keychain_writable().map_err(|e| {
            format!(
                "the Keychain rejected a test write ({e}); not claiming the token, \
                 since the Access URL could not be saved"
            )
        })?;
        let url = client
            .claim(&token)
            .await
            .map_err(|e| format!("claiming the Setup Token failed: {e}"))?;
        obol_core::store_simplefin_access_url(&url)
            .map_err(|e| format!("could not save the Access URL to the Keychain: {e}"))?;
        println!("Access URL saved to the Keychain.");
        url
    };

    let set = client
        .get_balances(&access_url)
        .await
        .map_err(|e| format!("fetching accounts failed: {e}"))?;
    for err in &set.errlist {
        eprintln!(
            "warning: [{}] {}",
            err.code,
            obol_core::sanitize_server_message(&err.msg)
        );
    }

    let accounts = obol_core::linkable_accounts(&set, &sources);
    if accounts.is_empty() {
        println!("No new accounts to link.");
        return Ok(());
    }

    let mut linked = 0;
    for account in accounts {
        println!(
            "\n{} — {} (balance {})",
            account.institution, account.name, account.balance
        );
        let default = match obol_core::default_category(&account.balance, &account.name) {
            Category::Liability => Choice::Liability,
            _ => Choice::Asset,
        };
        let category = match prompt_choice(default)? {
            Choice::Skip => continue,
            Choice::Asset => Category::Asset,
            Choice::Liability => Category::Liability,
        };
        let guess_type = obol_core::default_account_type(&account.name, category);
        let typed = prompt_line(&format!("  type [{guess_type}]: "))?.ok_or("input closed")?;
        let typed = obol_core::normalize_account_type(&typed);
        let account_type = if typed.is_empty() {
            guess_type.to_string()
        } else {
            typed
        };

        let source = obol_core::new_source(&account, category, &account_type, &sources);
        obol_core::add_source(sources_path, source.clone())
            .map_err(|e| format!("could not add source '{}': {e}", source.id))?;
        println!("  linked as '{}'", source.id);
        sources.push(source);
        linked += 1;
    }
    println!("\nLinked {linked} account(s).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn typed_characters_are_appended() {
        let mut buf = String::new();
        apply_key(&mut buf, press(KeyCode::Char('a')));
        apply_key(&mut buf, press(KeyCode::Char('b')));
        assert_eq!(buf, "ab");
    }

    #[test]
    fn shifted_characters_are_appended() {
        let mut buf = String::new();
        apply_key(
            &mut buf,
            KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        assert_eq!(buf, "A");
    }

    #[test]
    fn backspace_removes_the_last_character() {
        let mut buf = "abc".to_string();
        apply_key(&mut buf, press(KeyCode::Backspace));
        assert_eq!(buf, "ab");
    }

    #[test]
    fn backspace_on_an_empty_buffer_is_harmless() {
        let mut buf = String::new();
        let outcome = apply_key(&mut buf, press(KeyCode::Backspace));
        assert_eq!((buf.as_str(), outcome), ("", SecretKey::Pending));
    }

    #[test]
    fn enter_finishes_without_changing_the_buffer() {
        let mut buf = "tok".to_string();
        let outcome = apply_key(&mut buf, press(KeyCode::Enter));
        assert_eq!((buf.as_str(), outcome), ("tok", SecretKey::Done));
    }

    #[test]
    fn escape_cancels() {
        let mut buf = "tok".to_string();
        assert_eq!(
            apply_key(&mut buf, press(KeyCode::Esc)),
            SecretKey::Cancelled
        );
    }

    #[test]
    fn ctrl_c_cancels() {
        let mut buf = "tok".to_string();
        assert_eq!(apply_key(&mut buf, ctrl('c')), SecretKey::Cancelled);
    }

    #[test]
    fn other_control_combinations_are_not_typed() {
        let mut buf = String::new();
        apply_key(&mut buf, ctrl('a'));
        assert_eq!(buf, "");
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut buf = String::new();
        let release = KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        );
        apply_key(&mut buf, release);
        assert_eq!(buf, "");
    }

    #[test]
    fn non_text_keys_leave_the_buffer_alone() {
        let mut buf = "ab".to_string();
        let outcome = apply_key(&mut buf, press(KeyCode::Left));
        assert_eq!((buf.as_str(), outcome), ("ab", SecretKey::Pending));
    }

    #[test]
    fn blank_choice_takes_the_default() {
        assert_eq!(parse_choice("", Choice::Liability), Some(Choice::Liability));
    }

    #[test]
    fn whitespace_only_choice_takes_the_default() {
        assert_eq!(parse_choice("  ", Choice::Asset), Some(Choice::Asset));
    }

    #[test]
    fn asset_answers_are_recognized() {
        assert_eq!(parse_choice("A", Choice::Skip), Some(Choice::Asset));
        assert_eq!(parse_choice("asset", Choice::Skip), Some(Choice::Asset));
    }

    #[test]
    fn liability_answers_are_recognized() {
        assert_eq!(parse_choice("l", Choice::Asset), Some(Choice::Liability));
        assert_eq!(
            parse_choice(" Liability ", Choice::Asset),
            Some(Choice::Liability)
        );
    }

    #[test]
    fn skip_answers_are_recognized() {
        assert_eq!(parse_choice("s", Choice::Asset), Some(Choice::Skip));
        assert_eq!(parse_choice("SKIP", Choice::Asset), Some(Choice::Skip));
    }

    #[test]
    fn unrecognized_answers_are_rejected() {
        assert_eq!(parse_choice("maybe", Choice::Asset), None);
    }
}

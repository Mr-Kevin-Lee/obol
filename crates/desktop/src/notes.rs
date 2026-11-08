//! Your own notes on single transactions ("Dinner for Sam's birthday").
//! Desktop-only and kept in `transaction_notes.json`, apart from the month
//! files: a SimpleFIN sync rewrites those, and would drop a note stored
//! inside one. Keyed by source id, then transaction id.

use std::collections::BTreeMap;

use crate::dashboard::StorageLayout;

/// A sentence or two; enough for what a charge was, short enough to keep a
/// row readable.
pub const MAX_NOTE_CHARS: usize = 200;

/// Source id → transaction id → note.
pub type TransactionNotes = BTreeMap<String, BTreeMap<String, String>>;

pub fn read_notes(layout: &StorageLayout) -> Result<TransactionNotes, String> {
    match std::fs::read(&layout.transaction_notes) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| format!("transaction_notes.json could not be parsed: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TransactionNotes::new()),
        Err(err) => Err(format!("transaction_notes.json could not be read: {err}")),
    }
}

/// The note on one transaction, if it has one.
pub fn note_for(notes: &TransactionNotes, source_id: &str, id: &str) -> Option<String> {
    notes.get(source_id)?.get(id).cloned()
}

/// Sets one transaction's note, or clears it when `note` is blank. A
/// malformed file is reported and left as it is, never overwritten.
pub fn set_note(
    layout: &StorageLayout,
    source_id: &str,
    id: &str,
    note: &str,
) -> Result<(), String> {
    if source_id.trim().is_empty() || id.trim().is_empty() {
        return Err("a transaction note needs a transaction".to_string());
    }
    let note = note.trim();
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err(format!(
            "a transaction note can be at most {MAX_NOTE_CHARS} characters"
        ));
    }

    let mut notes = read_notes(layout)?;
    if note.is_empty() {
        if let Some(for_source) = notes.get_mut(source_id) {
            for_source.remove(id);
            if for_source.is_empty() {
                notes.remove(source_id);
            }
        }
    } else {
        notes
            .entry(source_id.to_string())
            .or_default()
            .insert(id.to_string(), note.to_string());
    }

    let json = serde_json::to_vec_pretty(&notes).map_err(|err| err.to_string())?;
    obol_core::write_atomically(&layout.transaction_notes, &json)
        .map_err(|err| format!("transaction_notes.json could not be saved: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_layout(name: &str) -> StorageLayout {
        let dir =
            std::env::temp_dir().join(format!("obol-notes-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        StorageLayout::under(&dir)
    }

    fn note_of(layout: &StorageLayout, source_id: &str, id: &str) -> Option<String> {
        note_for(&read_notes(layout).unwrap(), source_id, id)
    }

    #[test]
    fn should_read_no_notes_when_the_file_is_missing() {
        let layout = temp_layout("missing");

        assert_eq!(read_notes(&layout), Ok(TransactionNotes::new()));
    }

    #[test]
    fn should_round_trip_a_note() {
        let layout = temp_layout("round-trip");

        set_note(&layout, "chk", "t1", "Birthday dinner").unwrap();

        assert_eq!(
            note_of(&layout, "chk", "t1").as_deref(),
            Some("Birthday dinner")
        );
    }

    #[test]
    fn should_trim_surrounding_whitespace() {
        let layout = temp_layout("trim");

        set_note(&layout, "chk", "t1", "  Birthday dinner \n").unwrap();

        assert_eq!(
            note_of(&layout, "chk", "t1").as_deref(),
            Some("Birthday dinner")
        );
    }

    #[test]
    fn should_clear_a_note_set_to_blank() {
        let layout = temp_layout("clear");
        set_note(&layout, "chk", "t1", "Birthday dinner").unwrap();

        set_note(&layout, "chk", "t1", "   ").unwrap();

        assert_eq!(note_of(&layout, "chk", "t1"), None);
    }

    #[test]
    fn should_drop_a_source_once_its_last_note_is_cleared() {
        let layout = temp_layout("drop-source");
        set_note(&layout, "chk", "t1", "Birthday dinner").unwrap();

        set_note(&layout, "chk", "t1", "").unwrap();

        assert_eq!(read_notes(&layout), Ok(TransactionNotes::new()));
    }

    #[test]
    fn should_keep_other_transactions_notes() {
        let layout = temp_layout("keep-others");
        set_note(&layout, "chk", "t1", "One").unwrap();

        set_note(&layout, "chk", "t2", "Two").unwrap();

        assert_eq!(note_of(&layout, "chk", "t1").as_deref(), Some("One"));
    }

    #[test]
    fn should_keep_the_same_id_apart_across_sources() {
        let layout = temp_layout("sources");
        set_note(&layout, "chk", "t1", "Checking note").unwrap();

        set_note(&layout, "card", "t1", "Card note").unwrap();

        assert_eq!(
            note_of(&layout, "chk", "t1").as_deref(),
            Some("Checking note")
        );
    }

    #[test]
    fn should_replace_an_existing_note() {
        let layout = temp_layout("replace");
        set_note(&layout, "chk", "t1", "Old").unwrap();

        set_note(&layout, "chk", "t1", "New").unwrap();

        assert_eq!(note_of(&layout, "chk", "t1").as_deref(), Some("New"));
    }

    #[test]
    fn should_accept_a_note_of_exactly_the_maximum_length() {
        let layout = temp_layout("max-ok");

        let result = set_note(&layout, "chk", "t1", &"a".repeat(MAX_NOTE_CHARS));

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn should_reject_a_note_over_the_maximum_length() {
        let layout = temp_layout("max-over");

        let result = set_note(&layout, "chk", "t1", &"a".repeat(MAX_NOTE_CHARS + 1));

        assert!(result.is_err());
    }

    #[test]
    fn should_count_length_in_characters_not_bytes() {
        let layout = temp_layout("chars");

        let result = set_note(&layout, "chk", "t1", &"é".repeat(MAX_NOTE_CHARS));

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn should_reject_a_blank_source_or_transaction_id() {
        let layout = temp_layout("blank-ids");

        assert!(
            set_note(&layout, " ", "t1", "x").is_err()
                && set_note(&layout, "chk", "", "x").is_err()
        );
    }

    #[test]
    fn should_report_a_malformed_notes_file() {
        let layout = temp_layout("malformed-read");
        fs::write(&layout.transaction_notes, "not json").unwrap();

        assert!(read_notes(&layout)
            .unwrap_err()
            .contains("transaction_notes.json"));
    }

    #[test]
    fn should_leave_a_malformed_notes_file_untouched_on_set() {
        let layout = temp_layout("malformed-set");
        fs::write(&layout.transaction_notes, "not json").unwrap();

        let result = set_note(&layout, "chk", "t1", "x");

        assert_eq!(
            (
                result.is_err(),
                fs::read_to_string(&layout.transaction_notes).unwrap()
            ),
            (true, "not json".to_string())
        );
    }
}

//! Your own names for accounts, so two accounts at the same bank can be told
//! apart ("Chase Bank" twice becomes "Joint checking" and "Travel card").
//! Desktop-only and kept in `account_labels.json`, keyed by source id, so
//! `sources.yaml` and the snapshots stay as the providers wrote them.

use std::collections::BTreeMap;

use crate::dashboard::StorageLayout;

/// Long enough for any real name; short enough to keep a row on one line.
pub const MAX_LABEL_CHARS: usize = 60;

pub fn read_labels(layout: &StorageLayout) -> Result<BTreeMap<String, String>, String> {
    match std::fs::read(&layout.account_labels) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| format!("account_labels.json could not be parsed: {err}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(err) => Err(format!("account_labels.json could not be read: {err}")),
    }
}

/// Sets one account's label, or clears it when `label` is blank. A
/// malformed file is reported and left as it is, never overwritten.
pub fn set_label(layout: &StorageLayout, source_id: &str, label: &str) -> Result<(), String> {
    if source_id.trim().is_empty() {
        return Err("an account label needs an account".to_string());
    }
    let label = label.trim();
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(format!(
            "an account label can be at most {MAX_LABEL_CHARS} characters"
        ));
    }

    let mut labels = read_labels(layout)?;
    if label.is_empty() {
        labels.remove(source_id);
    } else {
        labels.insert(source_id.to_string(), label.to_string());
    }

    let json = serde_json::to_vec_pretty(&labels).map_err(|err| err.to_string())?;
    obol_core::write_atomically(&layout.account_labels, &json)
        .map_err(|err| format!("account_labels.json could not be saved: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_layout(name: &str) -> StorageLayout {
        let dir =
            std::env::temp_dir().join(format!("obol-labels-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        StorageLayout::under(&dir)
    }

    #[test]
    fn should_read_no_labels_when_the_file_is_missing() {
        let layout = temp_layout("missing");

        assert_eq!(read_labels(&layout), Ok(BTreeMap::new()));
    }

    #[test]
    fn should_round_trip_a_label() {
        let layout = temp_layout("round-trip");

        set_label(&layout, "chase_1", "Joint checking").unwrap();

        assert_eq!(
            read_labels(&layout)
                .unwrap()
                .get("chase_1")
                .map(String::as_str),
            Some("Joint checking")
        );
    }

    #[test]
    fn should_trim_surrounding_whitespace() {
        let layout = temp_layout("trim");

        set_label(&layout, "chase_1", "  Travel card \n").unwrap();

        assert_eq!(
            read_labels(&layout)
                .unwrap()
                .get("chase_1")
                .map(String::as_str),
            Some("Travel card")
        );
    }

    #[test]
    fn should_clear_a_label_set_to_blank() {
        let layout = temp_layout("clear");
        set_label(&layout, "chase_1", "Joint checking").unwrap();

        set_label(&layout, "chase_1", "   ").unwrap();

        assert_eq!(read_labels(&layout), Ok(BTreeMap::new()));
    }

    #[test]
    fn should_keep_other_accounts_labels() {
        let layout = temp_layout("keep-others");
        set_label(&layout, "chase_1", "Joint checking").unwrap();

        set_label(&layout, "chase_2", "Travel card").unwrap();

        assert_eq!(read_labels(&layout).unwrap().len(), 2);
    }

    #[test]
    fn should_replace_an_existing_label() {
        let layout = temp_layout("replace");
        set_label(&layout, "chase_1", "Old").unwrap();

        set_label(&layout, "chase_1", "New").unwrap();

        assert_eq!(
            read_labels(&layout)
                .unwrap()
                .get("chase_1")
                .map(String::as_str),
            Some("New")
        );
    }

    #[test]
    fn should_accept_a_label_of_exactly_the_maximum_length() {
        let layout = temp_layout("max-ok");

        let result = set_label(&layout, "chase_1", &"a".repeat(MAX_LABEL_CHARS));

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn should_reject_a_label_over_the_maximum_length() {
        let layout = temp_layout("max-over");

        let result = set_label(&layout, "chase_1", &"a".repeat(MAX_LABEL_CHARS + 1));

        assert!(result.is_err());
    }

    #[test]
    fn should_count_length_in_characters_not_bytes() {
        let layout = temp_layout("chars");

        let result = set_label(&layout, "chase_1", &"é".repeat(MAX_LABEL_CHARS));

        assert_eq!(result, Ok(()));
    }

    #[test]
    fn should_reject_a_blank_source_id() {
        let layout = temp_layout("blank-id");

        let result = set_label(&layout, " ", "Joint checking");

        assert!(result.is_err());
    }

    #[test]
    fn should_report_a_malformed_labels_file() {
        let layout = temp_layout("malformed-read");
        fs::write(&layout.account_labels, "not json").unwrap();

        assert!(read_labels(&layout)
            .unwrap_err()
            .contains("account_labels.json"));
    }

    #[test]
    fn should_leave_a_malformed_labels_file_untouched_on_set() {
        let layout = temp_layout("malformed-set");
        fs::write(&layout.account_labels, "not json").unwrap();

        let result = set_label(&layout, "chase_1", "Joint checking");

        assert_eq!(
            (
                result.is_err(),
                fs::read_to_string(&layout.account_labels).unwrap()
            ),
            (true, "not json".to_string())
        );
    }
}

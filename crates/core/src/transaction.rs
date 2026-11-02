//! Transactions (spec §11.4, D44): a separate store from snapshots, so
//! FR12's PII-free snapshots are untouched. A description can name a
//! person, so transactions are a sensitive file class of their own and are
//! never written to a log line.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use time::{Date, Month};

use crate::{Credentials, ProviderError, SourceConfig};

/// One posted transaction. There is deliberately no `pending` field:
/// providers skip pending transactions, whose amount and description can
/// still change on posting (#38), so storage can never hold one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    /// The obol source (`sources.yaml` id) this belongs to.
    pub source_id: String,
    /// The provider's id, unique within `source_id`.
    pub id: String,
    #[serde(with = "iso_date")]
    pub posted: Date,
    /// Signed: negative is money out (D16's `f64`).
    pub amount: f64,
    /// Raw, exactly as the provider gave it.
    pub description: String,
}

/// The provider-neutral transaction feed (#38), separate from `Provider`
/// (§10) so balance-only providers needn't implement it. SimpleFIN's
/// implementation arrives with #38, after #37 merges.
#[async_trait]
pub trait TransactionProvider: Send + Sync {
    /// Posted transactions for `source` on or after `since`. Implementations
    /// skip pending ones.
    async fn fetch_transactions(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
        since: Date,
    ) -> Result<Vec<Transaction>, ProviderError>;
}

/// A calendar month — the unit transactions are stored and planned in.
/// Years are limited to 1–9999; ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    year: i32,
    month: u8,
}

impl YearMonth {
    pub fn new(year: i32, month: u8) -> Option<Self> {
        ((1..=9999).contains(&year) && (1..=12).contains(&month)).then_some(Self { year, month })
    }

    pub fn of(date: Date) -> Self {
        Self {
            year: date.year(),
            month: u8::from(date.month()),
        }
    }

    /// Parses exactly `YYYY-MM`.
    pub fn parse(text: &str) -> Option<Self> {
        let (year, month) = text.split_once('-')?;
        if !is_digits(year, 4) || !is_digits(month, 2) {
            return None;
        }
        Self::new(year.parse().ok()?, month.parse().ok()?)
    }

    pub fn year(self) -> i32 {
        self.year
    }

    /// 1–12.
    pub fn month(self) -> u8 {
        self.month
    }

    pub fn previous(self) -> Self {
        if self.month == 1 {
            Self {
                year: self.year - 1,
                month: 12,
            }
        } else {
            Self {
                month: self.month - 1,
                ..self
            }
        }
    }

    pub fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                month: self.month + 1,
                ..self
            }
        }
    }

    pub fn first_day(self) -> Date {
        let month = Month::try_from(self.month).expect("YearMonth holds a valid month");
        Date::from_calendar_date(self.year, month, 1).expect("YearMonth holds a valid year")
    }

    pub fn last_day(self) -> Date {
        self.next()
            .first_day()
            .previous_day()
            .expect("the first of any later month has a previous day")
    }

    pub fn contains(self, date: Date) -> bool {
        Self::of(date) == self
    }
}

impl fmt::Display for YearMonth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

/// `YYYY-MM-DD`, the only date format transactions use on disk and over IPC.
pub fn format_iso_date(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// Parses exactly `YYYY-MM-DD`, rejecting impossible dates like Feb 30.
pub fn parse_iso_date(text: &str) -> Option<Date> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || !is_digits(year, 4) || !is_digits(month, 2) || !is_digits(day, 2) {
        return None;
    }
    let month = Month::try_from(month.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(year.parse().ok()?, month, day.parse().ok()?).ok()
}

/// Exactly `len` ASCII digits — rules out signs, spaces and short fields
/// that `str::parse` would otherwise accept or pad around.
fn is_digits(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `#[serde(with)]` adapter so `posted` is stored as `YYYY-MM-DD`, using
/// only `time` features core already enables.
mod iso_date {
    use super::{format_iso_date, parse_iso_date};
    use serde::{de, Deserialize, Deserializer, Serializer};
    use time::Date;

    pub fn serialize<S: Serializer>(date: &Date, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_iso_date(*date))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_iso_date(&text)
            .ok_or_else(|| de::Error::custom(format!("invalid date {text:?}, expected YYYY-MM-DD")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ym(year: i32, month: u8) -> YearMonth {
        YearMonth::new(year, month).unwrap()
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    #[test]
    fn should_parse_a_year_month() {
        assert_eq!(YearMonth::parse("2026-10"), Some(ym(2026, 10)));
    }

    #[test]
    fn should_reject_malformed_year_months() {
        let inputs = [
            "2026-13",
            "2026-00",
            "2026-1",
            "26-10",
            "2026/10",
            "2026-10-01",
            "02026-10",
            // `str::parse` alone would accept a sign and read year 26.
            "+026-10",
            "",
        ];

        assert_eq!(inputs.map(YearMonth::parse), [None; 9]);
    }

    #[test]
    fn should_display_zero_padded() {
        assert_eq!(ym(2026, 1).to_string(), "2026-01");
    }

    #[test]
    fn should_round_trip_through_display_and_parse() {
        assert_eq!(
            YearMonth::parse(&ym(2026, 3).to_string()),
            Some(ym(2026, 3))
        );
    }

    #[test]
    fn should_step_back_across_a_year_boundary() {
        assert_eq!(ym(2026, 1).previous(), ym(2025, 12));
    }

    #[test]
    fn should_step_forward_across_a_year_boundary() {
        assert_eq!(ym(2025, 12).next(), ym(2026, 1));
    }

    #[test]
    fn should_step_within_a_year() {
        assert_eq!(
            (ym(2026, 5).previous(), ym(2026, 5).next()),
            (ym(2026, 4), ym(2026, 6))
        );
    }

    #[test]
    fn should_start_on_the_first() {
        assert_eq!(ym(2026, 10).first_day(), date(2026, Month::October, 1));
    }

    #[test]
    fn should_end_february_on_the_29th_in_a_leap_year() {
        assert_eq!(ym(2028, 2).last_day(), date(2028, Month::February, 29));
    }

    #[test]
    fn should_end_february_on_the_28th_in_a_common_year() {
        assert_eq!(ym(2026, 2).last_day(), date(2026, Month::February, 28));
    }

    #[test]
    fn should_end_december_on_the_31st() {
        assert_eq!(ym(2026, 12).last_day(), date(2026, Month::December, 31));
    }

    #[test]
    fn should_contain_only_dates_in_its_month() {
        let october = ym(2026, 10);

        assert_eq!(
            (
                october.contains(date(2026, Month::October, 31)),
                october.contains(date(2026, Month::November, 1)),
                october.contains(date(2025, Month::October, 15)),
            ),
            (true, false, false)
        );
    }

    #[test]
    fn should_find_the_month_of_a_date() {
        assert_eq!(YearMonth::of(date(2026, Month::October, 26)), ym(2026, 10));
    }

    #[test]
    fn should_order_chronologically() {
        assert!(ym(2025, 12) < ym(2026, 1));
    }

    #[test]
    fn should_reject_out_of_range_constructor_arguments() {
        assert_eq!(
            [
                YearMonth::new(2026, 0),
                YearMonth::new(2026, 13),
                YearMonth::new(0, 6),
                YearMonth::new(10_000, 6)
            ],
            [None; 4]
        );
    }

    #[test]
    fn should_format_an_iso_date_zero_padded() {
        assert_eq!(format_iso_date(date(2026, Month::March, 5)), "2026-03-05");
    }

    #[test]
    fn should_reject_an_impossible_calendar_date() {
        assert_eq!(parse_iso_date("2026-02-30"), None);
    }

    #[test]
    fn should_reject_malformed_iso_dates() {
        let inputs = [
            "10/26/2026",
            "2026-10-26T00:00:00Z",
            "2026-1-26",
            "2026-10",
            "2026-10-26-01",
            "02026-10-26",
            "+026-10-26",
            "",
        ];

        assert_eq!(inputs.map(parse_iso_date), [None; 8]);
    }

    #[test]
    fn should_serialize_posted_as_an_iso_date() {
        let transaction = Transaction {
            source_id: "chase_checking".into(),
            id: "t1".into(),
            posted: date(2026, Month::October, 26),
            amount: -22.18,
            description: "AMAZON".into(),
        };

        assert_eq!(
            serde_json::to_value(&transaction).unwrap()["posted"],
            json!("2026-10-26")
        );
    }

    #[test]
    fn should_round_trip_a_transaction_through_json() {
        let transaction = Transaction {
            source_id: "chase_checking".into(),
            id: "t1".into(),
            posted: date(2026, Month::October, 26),
            amount: -22.18,
            description: "AMAZON MKTPL*AB12CD".into(),
        };

        let json = serde_json::to_string(&transaction).unwrap();

        assert_eq!(
            serde_json::from_str::<Transaction>(&json).unwrap(),
            transaction
        );
    }

    #[test]
    fn should_reject_a_malformed_posted_date_on_load() {
        let json =
            r#"{"source_id":"a","id":"t","posted":"10/26/2026","amount":-1.0,"description":"x"}"#;

        assert!(serde_json::from_str::<Transaction>(json).is_err());
    }
}

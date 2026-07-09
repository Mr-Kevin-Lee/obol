//! `StatementParser` trait (spec §6.3, D28) — per-institution statement
//! text parsing, implemented once per institution (`chase.rs` is the
//! reference implementation; `vanguard.rs`/`fidelity.rs` followed once
//! real statement layouts were available to check against; more
//! institutions are added the same way — a new sibling module plus one
//! new match arm in [`parser_for`], with no changes needed here).

use crate::statement_import::apple_card::AppleCardStatementParser;
use crate::statement_import::chase::ChaseStatementParser;
use crate::statement_import::fidelity::FidelityStatementParser;
use crate::statement_import::morgan_stanley::MorganStanleyStatementParser;
use crate::statement_import::vanguard::VanguardStatementParser;
use crate::{Category, Holding};

/// What `StatementImportProvider::fetch` expects to find in a
/// statement, derived from the calling `SourceConfig` — lets a parser
/// disambiguate when a single statement covers multiple accounts at the
/// same institution.
pub struct ExpectedAccount {
    pub account_type: String,
    /// Free-form disambiguator, interpreted differently per
    /// institution: a last-4 digit string for bank/brokerage-style
    /// statements (Chase, Vanguard), or a plan/employer-name substring
    /// for statements with no account number at all (Fidelity
    /// NetBenefits has none — just a plan name like "Apple 401(k)
    /// Plan"). `None` means "there's only one account in this
    /// statement, don't disambiguate."
    pub account_hint: Option<String>,
}

/// A successfully parsed statement — one balance, for one account.
#[derive(Debug)]
pub struct ParsedStatement {
    pub balance: f64,
    pub as_of_date: String,
    /// Stable raw identifier for this account (last-4, a full account
    /// number, or a plan name if that's all a statement exposes) —
    /// hashed into an `account_key` by the caller
    /// (`hash_account_number`), never persisted raw.
    pub account_identifier: String,
    /// Asset or liability, determined from the statement's own content
    /// rather than any directory-naming convention — used by
    /// `discovery::discover_statement_sources` (spec D29) to decide
    /// `SourceConfig.category` for a newly auto-discovered source. Not
    /// consulted by `StatementImportProvider::fetch()`'s regular per-run
    /// path, since an existing source's category is already fixed at
    /// add-time, same as every other provider.
    pub category: Category,
    /// Individual positions within this account (spec D31) — empty for
    /// every layout except one that actually lists holdings (currently
    /// only Vanguard's Cash Plus/Brokerage layout). Deliberately a
    /// plain `Vec`, not `Option<Vec<_>>`, consistent with `balance: f64`
    /// above — an empty vec is already the "no holdings" state, no
    /// extra `None` branch needed at any call site.
    pub holdings: Vec<Holding>,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    /// The statement was recognized, but no account in it matched
    /// `ExpectedAccount` (wrong hint, or a genuinely different
    /// account).
    NoMatchingAccount,
    /// More than one account in the statement matched, and no hint was
    /// given to disambiguate.
    AmbiguousMatch,
    /// The text didn't match any pattern this parser recognizes at all
    /// — likely a parser bug or a statement layout change.
    UnrecognizedLayout(String),
}

/// Parses already-extracted plain text (never a PDF path/bytes — see
/// `pdf_text.rs`) into a balance + as-of date + account identifier.
/// Implementations stay pure/synchronous so they're trivially
/// unit-testable against string-literal fixtures, independent of
/// `pdf-extract`'s actual runtime behavior.
pub trait StatementParser: Send + Sync {
    /// Institution key this parser handles — self-documentation only;
    /// [`parser_for`] is the single source of truth for dispatch, so
    /// the two can't disagree.
    fn institution(&self) -> &'static str;

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError>;
}

/// Selects the `StatementParser` for a `SourceConfig.institution`
/// value (matched case-insensitively, since existing `institution`
/// values elsewhere in this codebase are capitalized for display —
/// e.g. `PlaidProvider`'s tests use `"Chase"` — while this is also used
/// as a dispatch key here). A plain match, not a dynamic registry: this
/// is a small, fixed, compile-time set of parsers, not a plugin system.
/// Adding another institution later means adding one new match arm
/// here, nothing else.
pub fn parser_for(institution: &str) -> Option<Box<dyn StatementParser>> {
    match institution.to_lowercase().as_str() {
        "chase" => Some(Box::new(ChaseStatementParser)),
        "vanguard" => Some(Box::new(VanguardStatementParser)),
        "fidelity" => Some(Box::new(FidelityStatementParser)),
        "applecard" | "apple card" => Some(Box::new(AppleCardStatementParser)),
        "morganstanley" | "morgan stanley" | "etrade" | "e*trade" => {
            Some(Box::new(MorganStanleyStatementParser))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chase_resolves_to_the_chase_parser() {
        let parser = parser_for("chase").unwrap();
        assert_eq!(parser.institution(), "chase");
    }

    #[test]
    fn vanguard_resolves_to_the_vanguard_parser() {
        let parser = parser_for("vanguard").unwrap();
        assert_eq!(parser.institution(), "vanguard");
    }

    #[test]
    fn fidelity_resolves_to_the_fidelity_parser() {
        let parser = parser_for("fidelity").unwrap();
        assert_eq!(parser.institution(), "fidelity");
    }

    #[test]
    fn apple_card_resolves_to_the_apple_card_parser() {
        let parser = parser_for("applecard").unwrap();
        assert_eq!(parser.institution(), "applecard");
    }

    #[test]
    fn apple_card_also_resolves_with_a_space() {
        assert!(parser_for("Apple Card").is_some());
    }

    #[test]
    fn morgan_stanley_resolves_to_the_morgan_stanley_parser() {
        let parser = parser_for("morganstanley").unwrap();
        assert_eq!(parser.institution(), "morganstanley");
    }

    #[test]
    fn morgan_stanley_also_resolves_via_etrade_aliases() {
        assert!(parser_for("Morgan Stanley").is_some());
        assert!(parser_for("etrade").is_some());
        assert!(parser_for("E*TRADE").is_some());
    }

    #[test]
    fn institution_matching_is_case_insensitive() {
        assert!(parser_for("Chase").is_some());
        assert!(parser_for("CHASE").is_some());
        assert!(parser_for("Vanguard").is_some());
        assert!(parser_for("Fidelity").is_some());
        assert!(parser_for("APPLECARD").is_some());
        assert!(parser_for("MORGANSTANLEY").is_some());
    }

    #[test]
    fn an_unrecognized_institution_resolves_to_none() {
        assert!(parser_for("some_bank_nobody_has_written_a_parser_for").is_none());
    }
}

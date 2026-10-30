//! Category resolution (spec §11.4, D44). Computed whenever transactions
//! are read, never stored on them, so a rule edit re-categorizes the past
//! as well as the future. No LLM or network call, ever (#35).
//!
//! **Contract (task 111b), implemented by task 114.** First match wins:
//!
//! 1. A rule in `merchant_rules.yaml`, matched on the exact merchant key.
//! 2. Transfer pairing: an equal and opposite amount (within a cent) on a
//!    *different* source, posted within 5 days; each transaction pairs at
//!    most once. Both sides become [`TRANSFER`].
//! 3. Built-in defaults: common merchants plus transfer keywords
//!    (`AUTOPAY`, `PAYMENT THANK YOU`…). Matched by **word prefix**: the
//!    longest table entry whose words are a prefix of the key's words
//!    (`UBER` matches `UBER` and `UBER TRIP`, but not `UBERRIMA`; `UBER
//!    EATS` beats `UBER` for the key `UBER EATS`).
//!    Built as a table so a large generated one — e.g. from OpenStreetMap's
//!    Name Suggestion Index, a planned follow-up — can replace the
//!    hand-picked one without a code change.
//! 4. [`UNCATEGORIZED`].

use std::collections::BTreeSet;

use serde::Serialize;

use crate::merchant::merchant_key;
use crate::merchant_rules_storage::MerchantRules;
use crate::transaction::Transaction;

pub const UNCATEGORIZED: &str = "Uncategorized";
pub const TRANSFER: &str = "Transfer";
pub const INCOME: &str = "Income";

/// A best-effort seed of Simplifi's top-level list; the Simplifi import adds
/// the user's real categories as rule extras.
const BASE_CATEGORIES: &[&str] = &[
    "Auto & Transport",
    "Bills & Utilities",
    "Dining & Drinks",
    "Education",
    "Entertainment",
    "Fees",
    "Fitness",
    "Gifts",
    "Groceries",
    "Health",
    "Home",
    "Kids",
    "Personal Care",
    "Pets",
    "Shopping",
    "Taxes",
    "Travel",
    INCOME,
    TRANSFER,
    UNCATEGORIZED,
];

/// Merchant-key prefix → category. Every key is already canonical, so it
/// still matches whatever the merchant normalizer does.
const DEFAULT_CATEGORIES: &[(&str, &str)] = &[
    ("AMAZON", "Shopping"),
    ("TARGET", "Shopping"),
    ("WALMART", "Shopping"),
    ("BEST BUY", "Shopping"),
    ("HOME DEPOT", "Home"),
    ("LOWES", "Home"),
    ("IKEA", "Home"),
    ("COSTCO", "Groceries"),
    ("SAFEWAY", "Groceries"),
    ("WHOLE FOODS", "Groceries"),
    ("TRADER JOES", "Groceries"),
    ("KROGER", "Groceries"),
    // Order doesn't matter: the longest matching prefix wins.
    ("UBER", "Auto & Transport"),
    ("LYFT", "Auto & Transport"),
    ("SHELL", "Auto & Transport"),
    ("CHEVRON", "Auto & Transport"),
    ("STARBUCKS", "Dining & Drinks"),
    ("CHIPOTLE", "Dining & Drinks"),
    ("MCDONALDS", "Dining & Drinks"),
    ("UBER EATS", "Dining & Drinks"),
    ("DOORDASH", "Dining & Drinks"),
    ("GRUBHUB", "Dining & Drinks"),
    ("NETFLIX", "Entertainment"),
    ("SPOTIFY", "Entertainment"),
    ("HULU", "Entertainment"),
    ("CVS", "Health"),
    ("WALGREENS", "Health"),
    ("AT&T", "Bills & Utilities"),
    ("PG&E", "Bills & Utilities"),
    ("COMCAST", "Bills & Utilities"),
    ("VERIZON", "Bills & Utilities"),
    ("XFINITY", "Bills & Utilities"),
    ("PLANET FITNESS", "Fitness"),
    ("PETCO", "Pets"),
    ("CHEWY", "Pets"),
    ("DELTA AIR", "Travel"),
    ("UNITED AIRLINES", "Travel"),
    ("AIRBNB", "Travel"),
    ("AUTOPAY", TRANSFER),
    ("PAYMENT THANK YOU", TRANSFER),
    ("ONLINE TRANSFER", TRANSFER),
    ("TRANSFER TO", TRANSFER),
    ("TRANSFER FROM", TRANSFER),
    ("CREDIT CARD PAYMENT", TRANSFER),
    ("PAYROLL", INCOME),
    ("DIRECT DEP", INCOME),
];

/// A transaction with its resolved merchant key and category.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategorizedTransaction {
    pub transaction: Transaction,
    pub merchant_key: String,
    pub category: String,
}

impl CategorizedTransaction {
    pub fn is_transfer(&self) -> bool {
        self.category == TRANSFER
    }
}

/// Every category the app offers, in display order: Simplifi's top-level
/// list (including [`INCOME`], [`TRANSFER`] and [`UNCATEGORIZED`]), then
/// any category that appears in `rules` but not in that list, sorted.
pub fn available_categories(rules: &MerchantRules) -> Vec<String> {
    let extras: BTreeSet<&str> = rules
        .rules
        .values()
        .map(String::as_str)
        .filter(|category| !BASE_CATEGORIES.contains(category))
        .collect();
    BASE_CATEGORIES
        .iter()
        .copied()
        .chain(extras)
        .map(String::from)
        .collect()
}

/// Resolves every transaction, returned in input order. Transfer pairing
/// looks across all of `transactions`, so callers pass adjacent months too
/// (a payment on the 31st pairs with one on the 2nd) and filter afterwards.
pub fn categorize(
    transactions: &[Transaction],
    rules: &MerchantRules,
) -> Vec<CategorizedTransaction> {
    let keys: Vec<String> = transactions
        .iter()
        .map(|transaction| merchant_key(&transaction.description))
        .collect();
    let mut resolved: Vec<Option<String>> = keys
        .iter()
        .map(|key| rules.rules.get(key).cloned())
        .collect();
    pair_transfers(transactions, &mut resolved);

    transactions
        .iter()
        .zip(keys)
        .zip(resolved)
        .map(|((transaction, key), category)| CategorizedTransaction {
            category: category
                .unwrap_or_else(|| default_category(&key).unwrap_or(UNCATEGORIZED).to_string()),
            transaction: transaction.clone(),
            merchant_key: key,
        })
        .collect()
}

/// Marks both sides of each transfer pair among the unresolved transactions.
///
/// Each one, in (posted, source_id, id) order, takes the first unpaired
/// match after it in that order. That is the closest-in-time match,
/// tie-broken by (source_id, id): an earlier unpaired match can't exist,
/// because it would have been processed first, found this one still free,
/// and paired.
fn pair_transfers(transactions: &[Transaction], resolved: &mut [Option<String>]) {
    let mut order: Vec<usize> = (0..transactions.len())
        .filter(|&index| is_pairable(&transactions[index]))
        .collect();
    order.sort_by_key(|&index| {
        let transaction = &transactions[index];
        (transaction.posted, &transaction.source_id, &transaction.id)
    });

    for (position, &earlier) in order.iter().enumerate() {
        if resolved[earlier].is_some() {
            continue;
        }
        let partner = order[position + 1..].iter().copied().find(|&later| {
            resolved[later].is_none()
                && is_transfer_pair(&transactions[earlier], &transactions[later])
        });
        if let Some(later) = partner {
            resolved[earlier] = Some(TRANSFER.to_string());
            resolved[later] = Some(TRANSFER.to_string());
        }
    }
}

/// A sub-cent line isn't money moving between accounts.
fn is_pairable(transaction: &Transaction) -> bool {
    transaction.amount.abs() >= 0.01
}

/// `later` is never posted before `earlier`.
fn is_transfer_pair(earlier: &Transaction, later: &Transaction) -> bool {
    earlier.source_id != later.source_id
        && (earlier.amount + later.amount).abs() < 0.01
        && (later.posted - earlier.posted).whole_days() <= 5
}

/// The longest table entry whose words are a prefix of `key`'s words.
fn default_category(key: &str) -> Option<&'static str> {
    DEFAULT_CATEGORIES
        .iter()
        .filter(|(prefix, _)| {
            key.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|&(_, category)| category)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    fn tx(source_id: &str, id: &str, day: u8, amount: f64, description: &str) -> Transaction {
        Transaction {
            source_id: source_id.into(),
            id: id.into(),
            posted: Date::from_calendar_date(2026, Month::October, day).unwrap(),
            amount,
            description: description.into(),
        }
    }

    fn rules(pairs: &[(&str, &str)]) -> MerchantRules {
        MerchantRules {
            rules: pairs
                .iter()
                .map(|(key, category)| (key.to_string(), category.to_string()))
                .collect(),
        }
    }

    fn categories(transactions: &[Transaction], merchant_rules: &MerchantRules) -> Vec<String> {
        categorize(transactions, merchant_rules)
            .into_iter()
            .map(|categorized| categorized.category)
            .collect()
    }

    fn uncategorized_pair_categories(first: Transaction, second: Transaction) -> Vec<String> {
        categories(&[first, second], &MerchantRules::default())
    }

    fn category_of(description: &str) -> String {
        categories(
            &[tx("a", "1", 1, -10.0, description)],
            &MerchantRules::default(),
        )
        .remove(0)
    }

    // --- available_categories ---

    #[test]
    fn should_offer_the_base_list_when_there_are_no_rules() {
        let expected = [
            "Auto & Transport",
            "Bills & Utilities",
            "Dining & Drinks",
            "Education",
            "Entertainment",
            "Fees",
            "Fitness",
            "Gifts",
            "Groceries",
            "Health",
            "Home",
            "Kids",
            "Personal Care",
            "Pets",
            "Shopping",
            "Taxes",
            "Travel",
            INCOME,
            TRANSFER,
            UNCATEGORIZED,
        ];

        assert_eq!(available_categories(&MerchantRules::default()), expected);
    }

    #[test]
    fn should_append_rule_categories_missing_from_the_base_list_sorted() {
        let merchant_rules = rules(&[("A", "Zoo"), ("B", "Charity"), ("C", "Groceries")]);

        let offered = available_categories(&merchant_rules);

        assert_eq!(offered[BASE_CATEGORIES.len()..], ["Charity", "Zoo"]);
    }

    #[test]
    fn should_list_a_category_used_by_several_rules_once() {
        let merchant_rules = rules(&[("A", "Charity"), ("B", "Charity")]);

        let offered = available_categories(&merchant_rules);

        assert_eq!(offered.len(), BASE_CATEGORIES.len() + 1);
    }

    // --- resolution order ---

    #[test]
    fn should_use_a_rule_over_a_default_for_the_same_merchant() {
        let transactions = [tx("a", "1", 1, -10.0, "AMAZON")];

        let result = categories(&transactions, &rules(&[("AMAZON", "Gifts")]));

        assert_eq!(result, ["Gifts"]);
    }

    #[test]
    fn should_use_a_rule_over_transfer_pairing() {
        let transactions = [
            tx("a", "1", 1, -50.0, "VENMO"),
            tx("b", "2", 1, 50.0, "VENMO"),
        ];

        let result = categories(&transactions, &rules(&[("VENMO", "Gifts")]));

        assert_eq!(result, ["Gifts", "Gifts"]);
    }

    #[test]
    fn should_not_pair_with_a_transaction_a_rule_already_resolved() {
        let transactions = [
            tx("a", "1", 1, -50.0, "VENMO"),
            tx("b", "2", 1, 50.0, "ZELLE"),
        ];

        let result = categories(&transactions, &rules(&[("VENMO", "Gifts")]));

        assert_eq!(result, ["Gifts", UNCATEGORIZED]);
    }

    #[test]
    fn should_use_transfer_pairing_over_a_default() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "AMAZON"),
            tx("b", "2", 1, 50.0, "AMAZON"),
        );

        assert_eq!(result, [TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_use_a_default_when_nothing_else_matches() {
        assert_eq!(category_of("AMAZON"), "Shopping");
    }

    #[test]
    fn should_fall_back_to_uncategorized() {
        assert_eq!(category_of("ACME WIDGETS"), UNCATEGORIZED);
    }

    #[test]
    fn should_match_a_rule_on_the_merchant_key() {
        let transactions = [tx("a", "1", 1, -10.0, "  acme widgets ")];

        let result = categories(&transactions, &rules(&[("ACME WIDGETS", "Home")]));

        assert_eq!(result, ["Home"]);
    }

    #[test]
    fn should_set_the_merchant_key_from_the_description() {
        let transactions = [tx("a", "1", 1, -10.0, " uber eats ")];

        let result = categorize(&transactions, &MerchantRules::default());

        assert_eq!(result[0].merchant_key, merchant_key(" uber eats "));
    }

    #[test]
    fn should_keep_the_transaction_unchanged() {
        let transaction = tx("a", "1", 1, -10.0, "AMAZON");

        let result = categorize(
            std::slice::from_ref(&transaction),
            &MerchantRules::default(),
        );

        assert_eq!(result[0].transaction, transaction);
    }

    #[test]
    fn should_preserve_input_order() {
        let transactions = [
            tx("a", "1", 3, -10.0, "STARBUCKS"),
            tx("a", "2", 1, -10.0, "AMAZON"),
            tx("a", "3", 2, -10.0, "ACME WIDGETS"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, ["Dining & Drinks", "Shopping", UNCATEGORIZED]);
    }

    #[test]
    fn should_return_nothing_for_no_transactions() {
        assert!(categorize(&[], &MerchantRules::default()).is_empty());
    }

    // --- transfer pairing ---

    #[test]
    fn should_not_pair_within_the_same_source() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("a", "2", 1, 50.0, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_not_pair_amounts_of_the_same_sign() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 1, -50.0, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_not_pair_zero_amounts() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, 0.0, "ACME"),
            tx("b", "2", 1, 0.0, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_not_pair_sub_cent_amounts() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -0.009, "ACME"),
            tx("b", "2", 1, 0.009, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_pair_a_one_cent_transfer() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -0.01, "ACME"),
            tx("b", "2", 1, 0.01, "ACME"),
        );

        assert_eq!(result, [TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_pair_five_days_apart() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 6, 50.0, "ACME"),
        );

        assert_eq!(result, [TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_not_pair_six_days_apart() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 7, 50.0, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_not_pair_six_days_apart_when_the_later_one_comes_first() {
        let result = uncategorized_pair_categories(
            tx("b", "2", 7, 50.0, "ACME"),
            tx("a", "1", 1, -50.0, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_pair_amounts_within_a_cent() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 1, 50.005, "ACME"),
        );

        assert_eq!(result, [TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_pair_a_shortfall_within_a_cent() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 1, 49.995, "ACME"),
        );

        assert_eq!(result, [TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_not_pair_amounts_more_than_a_cent_apart() {
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 1, 50.02, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_not_pair_amounts_exactly_a_cent_apart() {
        // 0.02 - 0.01 is exactly the f64 0.01, unlike most cent sums.
        let result = uncategorized_pair_categories(
            tx("a", "1", 1, -0.01, "ACME"),
            tx("b", "2", 1, 0.02, "ACME"),
        );

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED]);
    }

    #[test]
    fn should_pair_each_transaction_at_most_once() {
        let transactions = [
            tx("a", "1", 1, -100.0, "ACME"),
            tx("b", "2", 2, 100.0, "ACME"),
            tx("c", "3", 3, -100.0, "ACME"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, [TRANSFER, TRANSFER, UNCATEGORIZED]);
    }

    #[test]
    fn should_pair_in_posted_order_not_input_order() {
        // Taking the input order, the day-4 debit would claim the credit
        // first; posted order gives it to the day-1 debit, closer in time.
        let transactions = [
            tx("c", "3", 4, -100.0, "ACME"),
            tx("b", "2", 2, 100.0, "ACME"),
            tx("a", "1", 1, -100.0, "ACME"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, [UNCATEGORIZED, TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_pair_with_the_closest_candidate_in_time() {
        let transactions = [
            tx("a", "1", 1, -100.0, "ACME"),
            tx("b", "2", 4, 100.0, "ACME"),
            tx("c", "3", 2, 100.0, "ACME"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, [TRANSFER, UNCATEGORIZED, TRANSFER]);
    }

    #[test]
    fn should_break_a_tie_in_time_by_source_then_id() {
        let transactions = [
            tx("a", "1", 1, -100.0, "ACME"),
            tx("c", "2", 2, 100.0, "ACME"),
            tx("b", "9", 2, 100.0, "ACME"),
            tx("b", "3", 2, 100.0, "ACME"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, [TRANSFER, UNCATEGORIZED, UNCATEGORIZED, TRANSFER]);
    }

    #[test]
    fn should_process_same_day_transactions_by_source_then_id() {
        // Both debits are a day from the credit; the one sorting first by
        // (source, id) claims it.
        let transactions = [
            tx("c", "1", 1, -100.0, "ACME"),
            tx("a", "9", 1, -100.0, "ACME"),
            tx("a", "2", 1, -100.0, "ACME"),
            tx("b", "1", 2, 100.0, "ACME"),
        ];

        let result = categories(&transactions, &MerchantRules::default());

        assert_eq!(result, [UNCATEGORIZED, UNCATEGORIZED, TRANSFER, TRANSFER]);
    }

    #[test]
    fn should_report_a_paired_transaction_as_a_transfer() {
        let transactions = [
            tx("a", "1", 1, -50.0, "ACME"),
            tx("b", "2", 1, 50.0, "ACME"),
        ];

        let result = categorize(&transactions, &MerchantRules::default());

        assert!(result[0].is_transfer());
    }

    #[test]
    fn should_not_report_a_purchase_as_a_transfer() {
        let result = categorize(
            &[tx("a", "1", 1, -10.0, "AMAZON")],
            &MerchantRules::default(),
        );

        assert!(!result[0].is_transfer());
    }

    // --- defaults ---

    #[test]
    fn should_match_a_default_by_word_prefix() {
        assert_eq!(category_of("UBER TRIP"), "Auto & Transport");
    }

    #[test]
    fn should_not_match_a_default_inside_a_longer_word() {
        assert_eq!(category_of("UBERRIMA"), UNCATEGORIZED);
    }

    #[test]
    fn should_not_match_a_default_that_is_not_a_prefix() {
        assert_eq!(category_of("MY UBER"), UNCATEGORIZED);
    }

    #[test]
    fn should_prefer_the_longest_matching_default() {
        assert_eq!(category_of("UBER EATS"), "Dining & Drinks");
    }

    #[test]
    fn should_categorize_a_transfer_keyword_as_a_transfer() {
        assert_eq!(category_of("PAYMENT THANK YOU"), TRANSFER);
    }

    #[test]
    fn should_categorize_a_payroll_keyword_as_income() {
        assert_eq!(category_of("PAYROLL"), INCOME);
    }

    #[test]
    fn should_match_a_default_containing_an_ampersand() {
        assert_eq!(category_of("AT&T"), "Bills & Utilities");
    }

    #[test]
    fn should_match_pge_by_word_prefix() {
        assert_eq!(category_of("PG&E WEB ONLINE"), "Bills & Utilities");
    }

    #[test]
    fn should_hold_only_canonical_keys_in_the_defaults_table() {
        let non_canonical: Vec<&str> = DEFAULT_CATEGORIES
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| merchant_key(key) != *key)
            .collect();

        assert!(non_canonical.is_empty(), "{non_canonical:?}");
    }

    #[test]
    fn should_map_defaults_only_to_offered_categories() {
        let offered = available_categories(&MerchantRules::default());

        assert!(DEFAULT_CATEGORIES
            .iter()
            .all(|(_, category)| offered.iter().any(|name| name == category)));
    }
}

//! Asset-class bucketing for holdings-bearing accounts (spec D31) —
//! deliberately a pure, independently-testable function over already-
//! extracted `Holding` data, not baked into any statement parser or the
//! persisted schema. This means reclassifying a holding later (e.g.
//! improving the fund-detection heuristic) never requires a new
//! statement parse or a schema migration — it's a read-time
//! aggregation over data that's already there. It's also what makes
//! the "eventually per-ticker" view free later: the same `Holding` list
//! this module buckets coarsely today already carries per-symbol
//! detail, ready to render at finer granularity without any new
//! parsing work.
//!
//! **Revised against a real Vanguard Brokerage statement**: the
//! original heuristic (guessed before seeing real holdings) looked for
//! the literal phrase `"index fund"` or `"etf"`. A real account turned
//! out to hold Vanguard *mutual* index funds (e.g. `"VANGUARD BALANCED
//! INDEX ADMIRAL CL"`, `"VANGUARD TOTAL BOND MARKET INDEX ADMIRAL CL"`)
//! — every one contains `"INDEX"`, none contain the literal phrase
//! `"index fund"` or `"etf"`. The original heuristic would have
//! silently misclassified every one of them as an individual stock.
//! Broadened accordingly; the `Fund` label covers both ETFs and mutual
//! funds; the sign is the same for the risk this feature exists to
//! surface — a diversified fund isn't a concentration risk the way a
//! single stock is, regardless of which wrapper it comes in.

use std::collections::BTreeMap;

use crate::account::Holding;

/// Deliberately coarse for v1 — matches what the user actually asked
/// for (cash/fund/individual-stock), not an exhaustive taxonomy (no
/// bonds/options/etc. yet). Declaration order is also render order
/// (`bucket`'s `BTreeMap` sorts by this derived `Ord`), roughly
/// safest-to-riskiest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssetClass {
    Cash,
    Fund,
    Stock,
}

impl AssetClass {
    pub fn label(&self) -> &'static str {
        match self {
            AssetClass::Cash => "Cash",
            AssetClass::Fund => "Fund",
            AssetClass::Stock => "Individual Stock",
        }
    }
}

/// Best-effort classification from a holding's `description` text —
/// unverified against every possible real fund-naming convention beyond
/// what's actually been seen (Vanguard's own "INDEX ADMIRAL" mutual
/// fund naming), same "heuristic, refine as real data surfaces" caveat
/// as this project's other content-based heuristics (e.g. Chase's
/// credit-card liability detection). Anything not recognized as
/// cash-like or fund-like defaults to `Stock`, since an unrecognized
/// ticker/description is far more likely to be an individual equity
/// than a way-off default would be a fund.
pub fn classify(holding: &Holding) -> AssetClass {
    let description = holding.description.to_lowercase();
    if description.contains("money market")
        || description.contains("settlement")
        || description.contains("cash")
    {
        AssetClass::Cash
    } else if description.contains("etf")
        || description.contains("index")
        || description.contains("fund")
    {
        AssetClass::Fund
    } else {
        AssetClass::Stock
    }
}

/// Sums each holding's value into its asset-class bucket. Returns
/// buckets in a stable, class-sorted order (via `BTreeMap`, not
/// insertion order) so rendering is deterministic across runs. Classes
/// with no holdings are simply absent, not zero-valued entries.
pub fn bucket(holdings: &[Holding]) -> Vec<(AssetClass, f64)> {
    let mut totals: BTreeMap<AssetClass, f64> = BTreeMap::new();
    for holding in holdings {
        *totals.entry(classify(holding)).or_insert(0.0) += holding.value;
    }
    totals.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holding(description: &str, value: f64) -> Holding {
        Holding {
            symbol: "TEST".into(),
            description: description.into(),
            value,
        }
    }

    #[test]
    fn classifies_a_money_market_description_as_cash() {
        assert_eq!(
            classify(&holding("VANGUARD FEDERAL MONEY MARKET FUND", 100.0)),
            AssetClass::Cash
        );
    }

    #[test]
    fn classifies_a_settlement_fund_description_as_cash() {
        assert_eq!(
            classify(&holding("SETTLEMENT FUND", 100.0)),
            AssetClass::Cash
        );
    }

    #[test]
    fn classifies_an_etf_description_as_etf() {
        assert_eq!(
            classify(&holding("VANGUARD S&P 500 ETF", 100.0)),
            AssetClass::Fund
        );
    }

    #[test]
    fn classifies_an_index_fund_description_as_etf() {
        assert_eq!(
            classify(&holding("TOTAL STOCK MARKET INDEX FUND", 100.0)),
            AssetClass::Fund
        );
    }

    #[test]
    fn classifies_a_real_world_vanguard_index_admiral_mutual_fund_as_fund() {
        // The real pattern this heuristic was revised for: no literal
        // "index fund" or "etf" phrase, just "INDEX" embedded in a
        // mutual fund's own naming convention.
        assert_eq!(
            classify(&holding("VANGUARD TOTAL BOND MARKET INDEX ADMIRAL CL", 100.0)),
            AssetClass::Fund
        );
    }

    #[test]
    fn classifies_an_unrecognized_description_as_individual_stock() {
        assert_eq!(classify(&holding("APPLE INC", 100.0)), AssetClass::Stock);
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            classify(&holding("vanguard money market fund", 100.0)),
            AssetClass::Cash
        );
    }

    #[test]
    fn bucket_sums_values_within_the_same_class() {
        let holdings = vec![
            holding("APPLE INC", 100.0),
            holding("MICROSOFT CORP", 50.0),
            holding("VANGUARD S&P 500 ETF", 200.0),
        ];

        let buckets = bucket(&holdings);

        assert_eq!(
            buckets,
            vec![(AssetClass::Fund, 200.0), (AssetClass::Stock, 150.0)]
        );
    }

    #[test]
    fn bucket_omits_classes_with_no_holdings() {
        let holdings = vec![holding("APPLE INC", 100.0)];
        let buckets = bucket(&holdings);
        assert_eq!(buckets, vec![(AssetClass::Stock, 100.0)]);
    }

    #[test]
    fn bucket_of_no_holdings_is_empty() {
        assert_eq!(bucket(&[]), vec![]);
    }

    #[test]
    fn bucket_orders_classes_cash_then_etf_then_stock() {
        let holdings = vec![
            holding("APPLE INC", 1.0),
            holding("MONEY MARKET FUND", 1.0),
            holding("SOME ETF", 1.0),
        ];

        let buckets = bucket(&holdings);
        let order: Vec<AssetClass> = buckets.iter().map(|(class, _)| *class).collect();

        assert_eq!(order, vec![AssetClass::Cash, AssetClass::Fund, AssetClass::Stock]);
    }
}

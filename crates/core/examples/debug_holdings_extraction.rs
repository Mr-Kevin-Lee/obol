//! One-off diagnostic tool — runs a `StatementParser` against a real
//! statement PDF and reports holdings *counts and asset classes only*,
//! never symbols, descriptions, or dollar values. Exists to answer one
//! question during development: did the holdings-table extraction match
//! anything at all in this real statement? Never checked in as a
//! fixture, not wired into any provider.
//!
//! Usage: `cargo run -p obol-core --example debug_holdings_extraction -- <institution> <path-to-pdf>`

use std::env;
use std::path::Path;

use obol_core::{parser_for, ExpectedAccount};

fn main() {
    let mut args = env::args().skip(1);
    let institution = args
        .next()
        .expect("usage: debug_holdings_extraction <institution> <path-to-pdf>");
    let path = args
        .next()
        .expect("usage: debug_holdings_extraction <institution> <path-to-pdf>");

    let parser = parser_for(&institution).expect("no parser for that institution");
    let text = obol_core::extract_text(Path::new(&path)).expect("extraction failed");

    let expected = ExpectedAccount {
        account_type: "brokerage".to_string(),
        account_hint: None,
    };

    match parser.parse(&text, &expected) {
        Ok(parsed) => {
            println!("parse: OK");
            println!("category: {:?}", parsed.category);
            println!("holdings found: {}", parsed.holdings.len());
            for h in &parsed.holdings {
                println!("  class: {:?}", obol_core::classify(h));
            }
        }
        Err(err) => {
            println!("parse: ERROR — {err:?}");
        }
    }
}

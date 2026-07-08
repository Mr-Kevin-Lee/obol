//! One-off diagnostic tool (spec §6.3, D28) — extracts plain text from a
//! statement PDF and redacts every digit, so real account numbers,
//! balances, and dates never leave the machine while still showing the
//! statement's structural wording (field labels, section headers).
//! Never checked in as a fixture, never fed to anything beyond a human
//! eyeballing it — this exists purely to see real statement structure
//! without ever seeing (or reproducing) a real value.
//!
//! Usage: `cargo run -p obol-core --example redact_statement_text -- <path-to-pdf>`

use std::env;
use std::path::Path;

fn redact(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_digit() {
            result.push('#');
        } else {
            result.push(c);
        }
    }
    result
}

fn main() {
    let path = env::args()
        .nth(1)
        .expect("usage: redact_statement_text <path-to-pdf>");

    let text = obol_core::extract_text(Path::new(&path)).expect("extraction failed");
    println!("{}", redact(&text));
}

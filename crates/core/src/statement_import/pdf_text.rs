//! Plain-text extraction from PDF statement files (spec §6.3, D28). The
//! only module in `statement_import` that touches a real PDF file — the
//! per-institution parsers (`chase.rs` etc.) operate on already-extracted
//! text, never a path or PDF bytes, so they stay trivially testable
//! against string fixtures regardless of how extraction itself behaves.
//!
//! Uses `pdf-extract`'s plain-text-layer extraction — no OCR/rendering,
//! since these are digitally generated bank/brokerage statements, not
//! scanned images (§6.3's rationale for this dependency choice: smallest
//! viable dependency tree per §4).
//!
//! **`pdftotext` fallback (spec D33).** `pdf-extract` 0.12.0 — the
//! latest release, confirmed against its own unreleased source too —
//! only maps `MacRomanEncoding`/`MacExpertEncoding`/`WinAnsiEncoding` to
//! Unicode and hard-`panic!`s on anything else. A real Chase Checking
//! statement uses `SymbolEncoding`, which this crate has never
//! supported at all — not a bug a version bump fixes. Rather than patch
//! or fork `pdf-extract`, whenever it fails (by panic or by `Err`) this
//! module shells out to the `pdftotext` CLI (poppler-utils) as a
//! best-effort fallback, since poppler's own encoding handling is far
//! more complete. `pdftotext` isn't a Cargo dependency — if it isn't
//! installed (or fails for its own reasons), the fallback quietly
//! returns `None` and the caller reports `pdf-extract`'s original error,
//! so someone without poppler installed still gets a sensible message
//! rather than a confusing one about a tool they've never heard of.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("could not extract text from {path}: {source}")]
    Extraction {
        path: String,
        #[source]
        source: pdf_extract::OutputError,
    },
    /// `pdf-extract` doesn't just return `Err` for every malformed
    /// input — real-world statements have hit internal `panic!`s (e.g.
    /// an unhandled font encoding) that would otherwise unwind straight
    /// through this crate and crash the whole CLI process. Caught via
    /// `catch_unwind` and turned into an ordinary error so one
    /// unparseable statement can't take down a run that has nothing to
    /// do with it.
    #[error("could not extract text from {path}: pdf-extract panicked: {message}")]
    Panicked { path: String, message: String },
}

/// Extracts plain text from a PDF file, falling back to `pdftotext`
/// (see module doc comment, D33) whenever `pdf-extract` itself can't
/// handle this particular file's internals.
pub fn extract_text(path: &Path) -> Result<String, ExtractError> {
    let result = catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(path)));

    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(source)) => pdftotext_fallback(path).ok_or_else(|| ExtractError::Extraction {
            path: path.display().to_string(),
            source,
        }),
        Err(panic_payload) => {
            let message = panic_message(panic_payload);
            pdftotext_fallback(path).ok_or_else(|| ExtractError::Panicked {
                path: path.display().to_string(),
                message,
            })
        }
    }
}

/// Runs `pdftotext <path> -` (poppler-utils) and returns its stdout as
/// text. `None` — never an error of its own — whenever the binary isn't
/// installed, exits non-zero, or produces non-UTF-8 output; the caller
/// always has `pdf-extract`'s own error to fall back to reporting in
/// that case. Not covered by an automated test (would require bundling
/// or asserting on a real `pdftotext` install in CI); verified manually
/// against a real Chase Checking statement that trips the
/// `SymbolEncoding` gap in `pdf-extract` — same rendering-code-style
/// carve-out as this module's terminal setup/teardown (§5/D9).
fn pdftotext_fallback(path: &Path) -> Option<String> {
    let output = Command::new("pdftotext").arg(path).arg("-").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// Extracts a human-readable message from a caught panic's payload —
/// the exact `&'static str` / `String` downcast pattern documented for
/// `std::panic::catch_unwind`, since a panic payload can be either
/// depending on whether the original `panic!()` call needed formatting.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<&'static str>() {
        Ok(message) => message.to_string(),
        Err(payload) => match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(_) => "unknown panic payload".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/chase_statement_sample.pdf")
    }

    #[test]
    fn extracts_text_from_the_synthetic_chase_fixture() {
        let text = extract_text(&fixture_path()).unwrap();
        assert!(!text.is_empty());
        // Loosely asserts extraction produced statement-like content,
        // without pinning to chase.rs's exact parsing regexes — this
        // test validates the extraction step, not the parser.
        assert!(text.contains("CHASE"));
        assert!(text.contains("1,234.56"));
    }

    #[test]
    fn a_missing_file_produces_a_clear_error() {
        let err = extract_text(Path::new("/nonexistent/does-not-exist.pdf")).unwrap_err();
        assert!(matches!(err, ExtractError::Extraction { .. }));
    }

    #[test]
    fn a_panic_inside_extraction_is_caught_and_turned_into_an_error() {
        // Regression test: a real Chase credit-card statement made
        // pdf-extract panic ("unexpected encoding \"SymbolEncoding\"")
        // instead of returning an `Err`, which crashed the whole CLI
        // process before this fix. Simulates that shape directly rather
        // than depending on a specific malformed fixture PDF, which
        // would be fragile to pdf-extract's own internals changing.
        // Uses an interpolated `panic!` (not a bare string literal) —
        // matching pdf-extract's own `panic!("...{:?}", encoding)`
        // shape, whose payload is a `String`, not a `&'static str`.
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<String, pdf_extract::OutputError> {
            panic!("unexpected encoding {:?}", "SymbolEncoding");
        }));

        let err = match result {
            Ok(_) => panic!("expected the simulated closure to panic"),
            Err(payload) => ExtractError::Panicked {
                path: "test.pdf".to_string(),
                message: panic_message(payload),
            },
        };

        assert!(matches!(err, ExtractError::Panicked { .. }));
        assert!(err.to_string().contains("SymbolEncoding"));
    }

    #[test]
    fn a_non_pdf_file_produces_a_clear_error() {
        let path = std::env::temp_dir().join(format!(
            "obol-pdf-text-test-not-a-pdf-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "this is not a pdf").unwrap();

        let err = extract_text(&path).unwrap_err();
        assert!(matches!(err, ExtractError::Extraction { .. }));

        std::fs::remove_file(&path).ok();
    }
}

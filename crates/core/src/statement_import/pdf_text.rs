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

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("could not extract text from {path}: {source}")]
    Extraction {
        path: String,
        #[source]
        source: pdf_extract::OutputError,
    },
}

/// Extracts plain text from a PDF file.
pub fn extract_text(path: &Path) -> Result<String, ExtractError> {
    pdf_extract::extract_text(path).map_err(|source| ExtractError::Extraction {
        path: path.display().to_string(),
        source,
    })
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

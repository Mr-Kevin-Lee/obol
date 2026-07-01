.PHONY: check fmt lint test audit

check: fmt lint test

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

# Requires `cargo install cargo-audit` (one-time, not a project dependency).
audit:
	cargo audit

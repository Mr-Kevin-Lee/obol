.PHONY: check fmt lint test audit test-keychain

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

# The Keychain tests are #[ignore]d because they touch the real login
# Keychain (throwaway items only). The plain write works unsigned.
# FILTER narrows which tests run, same as cargo test's own filter arg.
FILTER ?= keychain
test-keychain:
	cargo test -p obol-core --lib -- --ignored $(FILTER)

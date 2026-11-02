.PHONY: check fmt lint test audit test-keychain desktop-dev

check: fmt lint test

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

# Requires `cargo install cargo-audit` (one-time, not a project dependency).
# The npm half covers the desktop frontend's dependency tree, which D43
# accepted into the project on condition it gets the same scrutiny as the
# Cargo one (spec §4, D21).
audit:
	cargo audit
	npm --prefix crates/desktop/ui audit

# Runs the Tauri desktop app against a committed fixture set rather than
# real data (D43's mock seam). FIXTURE is one of the directories under
# crates/desktop/tests/fixtures: typical, all-failed, or empty.
FIXTURE ?= typical
desktop-dev:
	scripts/desktop_dev.sh $(FIXTURE)

# The Keychain tests are #[ignore]d because they touch the real login
# Keychain (throwaway items only). The plain write works unsigned.
# FILTER narrows which tests run, same as cargo test's own filter arg.
FILTER ?= keychain
test-keychain:
	cargo test -p obol-core --lib -- --ignored $(FILTER)

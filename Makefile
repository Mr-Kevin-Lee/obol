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

# Runs the Tauri desktop app against the committed fixture snapshots
# rather than real data (D43's mock seam) — no network, no providers, no
# `~/Library/Application Support/obol` access. Requires
# `cargo install tauri-cli` (one-time, not a project dependency).
desktop-dev:
	cd crates/desktop && OBOL_STORAGE_DIR=$(CURDIR)/crates/desktop/tests/fixtures cargo tauri dev

# KNOWN BROKEN, PARKED (spec.md decision D24) — not a working recipe.
# Ignored tests that store a Keychain item with an explicit accessibility
# level (SecAccessControl) fail with errSecMissingEntitlement (-34018)
# unsigned. Ad-hoc signing with entitlements.plist's keychain-access-groups
# entitlement (attempted here) makes it worse — the process gets killed
# outright by the kernel before any code runs. Left in place as a
# documented starting point for whoever revisits this, not a fix.
# FILTER narrows which tests run, same as cargo test's own filter arg.
FILTER ?= keychain
test-keychain:
	cargo test -p obol-core --lib --no-run
	@bin=$$(find target/debug/deps -type f -perm +111 -name 'obol_core-*' ! -name '*.d' -print0 | xargs -0 ls -t | head -1); \
	echo "Signing $$bin"; \
	codesign --force --sign - --entitlements entitlements.plist $$bin; \
	$$bin --ignored $(FILTER)

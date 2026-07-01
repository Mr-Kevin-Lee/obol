#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml — run before pushing.
set -euo pipefail
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

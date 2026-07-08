#!/usr/bin/env bash
# Creates one GitHub issue per v0.1 task from docs/tasks.md, plus phase
# labels and a v0.1 milestone. Requires `gh` authenticated and this repo's
# remote configured. Run from the repo root.
#
#   bash scripts/create_github_issues.sh
#
set -euo pipefail

REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
echo "About to create labels + a v0.1 milestone + 27 issues in: $REPO"
read -r -p "Continue? [y/N] " confirm
if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
  echo "Aborted."
  exit 1
fi

# --- Labels (idempotent: ignore "already exists" errors) ---
create_label() {
  local name="$1" color="$2" desc="$3"
  gh label create "$name" --color "$color" --description "$desc" --force
}

create_label "phase-a-scaffolding"   "ededed" "Cargo workspace scaffolding"
create_label "phase-b-core-domain"   "1d76db" "Core domain & pure logic (test-first)"
create_label "phase-c-storage"       "0e8a16" "Storage & config (test-first)"
create_label "phase-d-orchestration" "5319e7" "Snapshot orchestration"
create_label "phase-e-manual-entry"  "fbca04" "Manual-entry provider"
create_label "phase-f-plaid"         "d93f0b" "Plaid integration"
create_label "phase-g-webdriver"     "c2e0c6" "WebDriver de-risking spike"
create_label "phase-h-cli-tui"       "006b75" "CLI / TUI"
create_label "phase-i-close-out"     "e99695" "Release & packaging"

# --- Milestone ---
gh api "repos/$REPO/milestones" -f title="v0.1" -f state="open" \
  -f description="Core library + CLI/TUI, per docs/spec.md §15" \
  >/dev/null 2>&1 || echo "Milestone v0.1 may already exist, continuing."

create_issue() {
  local num="$1" title="$2" label="$3" body="$4"
  gh issue create \
    --title "Task ${num}: ${title}" \
    --label "$label" \
    --milestone "v0.1" \
    --body "$body"
}

# --- Phase A ---
create_issue 1 "Cargo workspace scaffold" "phase-a-scaffolding" \
"Set up \`crates/core\` and \`crates/cli\`, committed \`Cargo.lock\`, lint/test config.

No business logic yet — no test requirement beyond \`cargo build\`/\`cargo test\` running clean on empty crates.

Spec: §6.1 (core has no UI deps), §14 (tech stack).
See docs/tasks.md, task 1."

# --- Phase B ---
create_issue 2 "Account trait + Asset/Liability structs" "phase-b-core-domain" \
"Implement the \`Account\` trait and \`Asset\`/\`Liability\` structs, \`AccountStatus\` (decision D11).

Tests (test-first): \`net_worth_contribution()\` sign for each variant.

Spec: §11, D11.
See docs/tasks.md, task 2."

create_issue 3 "Snapshot JSON schema DTOs + serde round-trip" "phase-b-core-domain" \
"Snapshot JSON schema DTOs, serde round-trip, mandatory \`schema_version\`.

Tests (test-first): serialize → deserialize → equality, for each account status variant.

Spec: §11.2.
See docs/tasks.md, task 3."

create_issue 4 "Backward/forward-compat migration chain" "phase-b-core-domain" \
"Migration chain (\`migrate_v1_to_v2\`, etc.) run in memory on load; stored files never rewritten. Also handles forward compatibility (decision D14) — unknown fields/newer schema versions parsed leniently.

Tests (test-first): fixture files per historical schema version load correctly; an unknown-field/newer-version fixture loads leniently.

Spec: §11.3, D14.
See docs/tasks.md, task 4."

create_issue 5 "PII scrubbing" "phase-b-core-domain" \
"Snapshots store no account numbers, holder names, login identifiers, or raw provider responses.

Tests (test-first): assert no PII appears in serialized output, across every provider's raw response shape.

Spec: §11.1.
See docs/tasks.md, task 5."

create_issue 6 "Net worth calculation" "phase-b-core-domain" \
"Net worth = sum(assets) − sum(liabilities) over \`status: ok\` accounts only, via \`.net_worth_contribution()\`. Include the all-sources-failed case (§9.1) — never render \`\$0\` when nothing succeeded.

Tests (test-first): mixed ok/error accounts, all-ok, all-error.

Spec: §12, §9.1.
See docs/tasks.md, task 6."

create_issue 7 "Retry/backoff wrapper (tokio-retry RetryIf)" "phase-b-core-domain" \
"3 attempts, exponential backoff starting at 2s doubling, ±20% jitter, 15s hard timeout per attempt, via \`tokio-retry\`'s \`RetryIf\` (decision D10). Auth failures fail fast, not retried.

Tests (test-first): attempt count, backoff timing, jitter bounds (\`proptest\`), auth errors fail fast without retrying.

Spec: §9, D10.
See docs/tasks.md, task 7."

create_issue 8 "Provider trait + registry" "phase-b-core-domain" \
"\`Provider\` trait and \`provider_registry()\` (plaid / webdriver / manual_entry factories).

Tests (test-first): contract tests against fake/in-memory providers.

Spec: §10, §5.
See docs/tasks.md, task 8."

create_issue 9 "Plaid Item usage counter" "phase-b-core-domain" \
"\`plaid_items_created_lifetime\` counter: increments on Item creation, never decrements on removal, blocks new Plaid connections at 10/10, warns at 8/10 (decision D8).

Tests (test-first): increments on creation, never decrements on removal, blocks at 10/10.

Spec: §7.1, D8.
See docs/tasks.md, task 9."

# --- Phase C ---
create_issue 10 "Snapshot storage (atomic write, permissions)" "phase-c-storage" \
"Save/load snapshots, atomic write (temp file + rename), \`0600\` files / \`0700\` directory.

Tests (test-first): round-trip, permission bits, crash-mid-write simulation doesn't corrupt the prior file.

Spec: §11.2, §4.
See docs/tasks.md, task 10."

create_issue 11 "Sources config CRUD (sources.yaml)" "phase-c-storage" \
"Load/save \`sources.yaml\`, atomic write, per-source \`account_salt\` generation at add-time (decision D15), clear parse-error message on a malformed file (§9.1) rather than silently falling back.

Tests (test-first): add/edit/remove, atomic-write behavior, parse-error message on a deliberately broken fixture.

Spec: §10.1, §9.1, D15.
See docs/tasks.md, task 11."

create_issue 12 "Cross-process file lock" "phase-c-storage" \
"Advisory file lock (\`fs2\`/\`fslock\`, \`flock\`) around write-critical sections — config writes, snapshot writes, Item counter increments — so a scheduled run and an interactive session don't race (decision D13).

Tests (test-first): second acquisition blocks/times out while the first holds the lock.

Spec: §9.1, D13.
See docs/tasks.md, task 12."

# --- Phase D ---
create_issue 13 "CredentialSource trait + snapshot engine orchestration" "phase-d-orchestration" \
"\`CredentialSource\` trait (decision D12) and \`core::snapshot::run()\`: providers deduped and instantiated once per type, concurrent per-source fetch, credential resolution (Keychain for Plaid, callback for webdriver/manual_entry), retry-wrapped fetch, PII-scrubbed assembly.

Tests (test-first): fake providers + fake \`CredentialSource\`, provider dedup by type, concurrent fetch, PII-scrubbed assembly.

Spec: §6.2, D12.
See docs/tasks.md, task 13."

create_issue 14 "Failure-mode wiring" "phase-d-orchestration" \
"On top of task 13: best-effort (non-blocking) snapshot persistence, Plaid Keychain failure treated as a relink signal, unknown-provider entries isolated to that source only (decision D13).

Tests (test-first): simulated failures for each case, verifying the run still completes and other sources are unaffected.

Spec: §9.1.
See docs/tasks.md, task 14."

# --- Phase E ---
create_issue 15 "ManualEntryProvider + CLI CredentialSource impl" "phase-e-manual-entry" \
"\`ManualEntryProvider\` (Apple Card, decision D3) and the CLI's \`CredentialSource\` implementation (masked terminal prompt for webdriver creds / manual balance entry).

Tests: \`ManualEntryProvider\` unit-tested like any other \`Provider\`; the terminal-prompt \`CredentialSource\` impl verified manually rather than unit-testing terminal I/O.

Spec: §10, §8, D3.
See docs/tasks.md, task 15."

# --- Phase F ---
create_issue 16 "Hand-rolled Plaid REST client" "phase-f-plaid" \
"\`reqwest\`/\`serde\` client against Plaid's documented endpoints: Balance, Investments, Liabilities, Link.

Tests: integration tier, against Plaid Sandbox — not unit tests (§5).

Spec: §5, §7, §14.
See docs/tasks.md, task 16."

create_issue 17 "Keychain token storage wrapper" "phase-f-plaid" \
"\`security-framework\`-based wrapper storing the Plaid access token under \`kSecAttrAccessibleWhenUnlockedThisDeviceOnly\`, scoped to this app's access group, never iCloud-synced.

Tests: store/read/delete round-trip against a real (test) Keychain entry.

Spec: §8, §4.
See docs/tasks.md, task 17."

create_issue 18 "PlaidProvider" "phase-f-plaid" \
"Ties the REST client (task 16) and Keychain wrapper (task 17) into the \`Provider\` trait.

Tests: unit tests against a fake HTTP layer for the \`Provider\` contract; real-network path covered by task 16's integration tier.

Spec: §10.
See docs/tasks.md, task 18."

create_issue 19 "Plaid Hosted Link connect flow" "phase-f-plaid" \
"\`/link/token/create\` → display URL/QR → background, non-blocking, cancelable polling of \`/link/token/get\` (decision D18) → exchange \`public_token\` → store in Keychain → increment Item counter → write source entry + \`account_salt\`.

Tests: unit-test the token-exchange/Item-counter/source-write logic; the actual Link session verified manually (hosted, browser-driven flow).

Spec: §10.1, D18.
See docs/tasks.md, task 19."

create_issue 20 "Plaid source removal flow" "phase-f-plaid" \
"\`/item/remove\` before deleting the config entry, releasing the Item and ending subscription billing; delete the corresponding Keychain entry in the same step; confirmed UI action, not a silent side effect.

Tests: \`/item/remove\` + Keychain cleanup unit-tested against fakes.

Spec: §10.1.
See docs/tasks.md, task 20."

# --- Phase G ---
create_issue 21 "fantoccini spike against a real bank-style login" "phase-g-webdriver" \
"Short, isolated spike validating \`fantoccini\`/WebDriver against one real (non-critical) bank-style login flow before committing to browser automation for v0.2. Go/no-go checkpoint.

Tests: none — verified manually, per §5's explicit carve-out for live third-party integration.

Spec: §14, §15, §5.
See docs/tasks.md, task 21."

# --- Phase H ---
create_issue 22 "clap command skeleton + first-run branch" "phase-h-cli-tui" \
"\`obol\` (default, interactive), \`obol snapshot\` (headless), \`obol sources\` subcommands. First-run branch: missing/empty \`sources.yaml\` creates an empty one and opens directly to the Sources screen (§10.1) rather than running a snapshot against an empty list.

Tests: unit-test the first-run detection/branch logic; command dispatch verified manually.

Spec: §6.2, §10.1.
See docs/tasks.md, task 22."

create_issue 23 "Dashboard screen (ratatui)" "phase-h-cli-tui" \
"Per-source panels (clear 'unavailable' state on failure), top-level net worth figure (incl. the explicit unavailable state, §9.1/§12), assets/liabilities grouped separately, Okabe–Ito colorblind-friendly palette with status never conveyed by color alone.

Tests: none — manual verification against the running TUI, per §5.

Spec: §13.
See docs/tasks.md, task 23."

create_issue 24 "Sources screen — list, health, Item indicator, generic forms" "phase-h-cli-tui" \
"List of configured sources with provider/category/type/connection health; persistent Plaid Item usage indicator ('Plaid Items: X/10 used', warn at 8/10); add/edit/remove forms generated from each provider's declared config schema (manual_entry, webdriver cases).

Tests: form validation logic unit-tested; rendering verified manually.

Spec: §13, §7.1, §10.1.
See docs/tasks.md, task 24."

create_issue 25 "Sources screen — Plaid Hosted Link UI" "phase-h-cli-tui" \
"UI for the 'Connect via Plaid' flow: progress display, URL/QR code, cancel option for a pending session. Wraps task 19's core logic.

Tests: none — manual verification (hosted, browser-driven flow).

Spec: §10.1, §13.
See docs/tasks.md, task 25."

create_issue 26 "Audit logging (tracing)" "phase-h-cli-tui" \
"Minimal local run history via \`tracing\`/\`tracing-subscriber\`: timestamp, which sources succeeded/failed — no balances, no identifiers, ever.

Tests: unit-test that log output never contains a credential, balance, or account number, using tracing's test subscriber.

Spec: §4.
See docs/tasks.md, task 26."

# --- Phase I ---
create_issue 27 "Release build + packaging check + e2e walkthrough" "phase-i-close-out" \
"\`cargo build --release\`, confirm no runtime/interpreter dependency, full manual walkthrough of the v0.1 flow end to end (first run → add sources → snapshot → dashboard → Sources screen management).

Tests: none — manual acceptance pass for v0.1 as a whole.

Spec: §14, §15.
See docs/tasks.md, task 27."

echo "Done. Created labels, milestone v0.1, and 27 issues in $REPO."

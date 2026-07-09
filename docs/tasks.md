# Implementation Tasks — v0.1

Source of record: [spec.md](spec.md). This breaks v0.1 down into
commit-sized tasks, ordered by dependency across phases. Phases are
ordering groups, not strict gates — Phase G (WebDriver spike) can run in
parallel with E/F rather than waiting on them.

**Ground rules:**

- Every task lands as **at least one commit** — task and commit
  boundaries should line up. Don't bundle multiple tasks into one commit,
  and don't leave a task half-committed.
- Every task includes tests for the change it introduces. Which kind
  follows the test-first split already established in §5 / D9 of the
  spec:
  - **Core library logic** (Phases B–F, except where noted) — the failing
    test is written *before* the implementation; `cargo test`, offline
    and deterministic.
  - **Real external systems** (Plaid Sandbox calls in task 16, the
    fantoccini spike in task 21) — integration-tested separately, not
    forced into the same red-green loop (§5).
  - **TUI rendering** (Phase H's screen tasks) — no unit-test mandate,
    consistent with §5 scoping test-first to core library logic only;
    verify manually against the running TUI. Non-rendering logic backing
    those screens (e.g. form validation) still gets unit tests.

## Phase A — Scaffolding

1. Cargo workspace (`crates/core`, `crates/cli`), lockfile, lint/test
   config. No logic yet — no test requirement beyond `cargo build`/
   `cargo test` running clean on an empty crate.

## Phase B — Core domain & pure logic (test-first)

2. `Account` trait + `Asset`/`Liability` structs, `AccountStatus` (D11).
   Tests: `net_worth_contribution()` sign for each variant.
3. Snapshot JSON schema DTOs + serde round-trip, `schema_version` (§11.2).
   Tests: serialize → deserialize → equality, for each account status
   variant.
4. Backward/forward-compat migration chain (§11.3, D14).
   Tests: fixture files per historical schema version load correctly;
   an unknown-field/newer-version fixture loads leniently.
5. PII scrubbing (§11.1).
   Tests: assert no account number, name, or raw payload appears in
   serialized output, across every provider's raw response shape.
6. Net worth calculation, including the all-sources-failed case (§12, §9.1).
   Tests: mixed ok/error accounts, all-ok, all-error.
7. Retry/backoff wrapper — `tokio-retry`'s `RetryIf` (§9, D10).
   Tests: attempt count, backoff timing, jitter bounds (`proptest`), auth
   errors fail fast without retrying.
8. `Provider` trait + registry (§10, §5).
   Tests: contract tests against fake/in-memory providers.
9. Plaid Item usage counter (§7.1).
   Tests: increments on creation, never decrements on removal, blocks at
   10/10.

## Phase C — Storage & config (test-first)

10. Snapshot storage — save/load, atomic write, `0600`/`0700` perms
    (§11.2, §4).
    Tests: round-trip, permission bits, crash-mid-write simulation
    doesn't corrupt the prior file.
11. Sources config CRUD — `sources.yaml` load/save, atomic write,
    `account_salt` generation (D15), malformed-file error (§9.1).
    Tests: add/edit/remove, atomic-write behavior, parse-error message on
    a deliberately broken fixture.
12. Cross-process file lock (`fs2`/`fslock`) (§9.1, D13).
    Tests: second acquisition blocks/times out while the first holds the
    lock.

## Phase D — Orchestration

13. `CredentialSource` trait + `core::snapshot::run()` (§6.2, D12).
    Tests: fake providers + fake `CredentialSource`, provider dedup by
    type, concurrent fetch, PII-scrubbed assembly.
14. Failure-mode wiring (§9.1): best-effort snapshot persistence,
    Keychain-failure→relink signal, unknown-provider isolation.
    Tests: simulated failures for each case, verifying the run still
    completes and other sources are unaffected.

## Phase E — Simplest real provider

15. `ManualEntryProvider` + CLI's `CredentialSource` impl. **Never
    actually built** — `NoInteractiveProvidersYet` (a placeholder that
    always declines) has stood in for it since Phase H, and the
    `manual_entry` provider string was never wired to a real factory
    (`provider.rs`'s own doc comment says so). The Sources screen form
    still lists `manual_entry` as a choice, so picking it produces an
    "unknown provider" error at fetch time — a real gap for as long as
    anything actually needed it. **Parked indefinitely as of D30**: the
    one account this existed for (Apple Card) turned out to have a real
    downloadable statement after all, so it uses `statement_import`
    instead (task 42) and nothing in this app's current scope needs
    `ManualEntryProvider` anymore. Revisit only if a genuinely
    statement-less, aggregator-less institution shows up later.
    Tests: `ManualEntryProvider` unit-tested like any other `Provider`;
    the terminal-prompt `CredentialSource` impl is thin enough to verify
    manually rather than unit-test actual terminal I/O.

## Phase F — Plaid

16. Hand-rolled Plaid REST client (Balance/Investments/Liabilities)
    (§5, §7).
    Tests: integration tier, against Plaid Sandbox — not unit tests.
17. Keychain token storage wrapper (`security-framework`) (§8).
    Tests: store/read/delete round-trip against a real (test) Keychain
    entry.
18. `PlaidProvider` tying 16+17 into the `Provider` trait.
    Tests: unit tests against a fake HTTP layer for the `Provider`
    contract; the real-network path is covered by 16's integration tier.
19. Plaid Hosted Link connect flow (§10.1, D18).
    Tests: unit-test the token-exchange/Item-counter/source-write logic;
    the actual Link session is verified manually (it's a hosted,
    browser-driven flow).
20. Plaid source removal flow (§10.1).
    Tests: `/item/remove` + Keychain cleanup unit-tested against fakes.

## Phase G — WebDriver de-risking (can run in parallel with E/F)

21. `fantoccini` spike (§14, §15). Go/no-go checkpoint, not TDD — verified
    manually against a real login flow, per §5's explicit carve-out.

## Phase H — CLI/TUI

22. `clap` command skeleton + first-run branch (§6.2, §10.1).
    Tests: unit-test the first-run detection/branch logic; command
    dispatch verified manually.
23. Dashboard screen (§13). Manual verification against the running TUI.
24. Sources screen — list/health/Item indicator/generic forms (§13).
    Tests: form validation logic unit-tested; rendering verified manually.
25. Sources screen — Plaid Hosted Link UI, wraps task 19. Manual
    verification.
26. Audit logging (`tracing`) (§4).
    Tests: unit-test that log output never contains a credential,
    balance, or account number, using `tracing`'s test subscriber.

## Phase I — Close-out

27. Release build + packaging check, full manual end-to-end walkthrough.
    No new unit tests — this is the manual acceptance pass for v0.1 as a
    whole.

## Phase J — Statement dropbox provider (Chase reference implementation)

Decided post-v0.1 (D28, §6.3): a third `Provider` alongside Plaid and
WebDriver — parses balances out of PDF statements dropped into a
per-source directory, as a parked/parallel alternative for institutions
where live automation isn't practical. Chase is the reference
implementation; Vanguard/Morgan Stanley/Fidelity are independent
follow-on work, not part of this phase.

28. `ProcessedFilesLedger` data model (mirrors `item_usage.rs`).
    Tests: is_processed/mark_processed round-trip, last-known-balance
    retained when nothing new is found, per-source isolation.
29. `statement_import_storage.rs` — load_or_init/save, atomic write,
    `0600` (mirrors `item_usage_storage.rs`).
    Tests: round-trip, permission bits, malformed-file parse error.
30. `pdf_text.rs` — `pdf-extract` wrapper.
    Tests: one checked-in synthetic fixture PDF, extraction succeeds and
    contains expected substrings; missing-file and not-a-PDF cases.
31. `StatementParser` trait + `ChaseStatementParser` — regex/heuristic
    extraction of balance + as-of date + account identifier.
    Tests: string-literal fixtures — single matching account, multiple
    accounts + last4 disambiguation, no matching account, ambiguous match,
    unrecognized layout. Test-first (§5/D9) — deterministic, offline, no
    PDF I/O in this module.
32. `parser_for(institution)` dispatch.
    Tests: "chase" resolves; unrecognized institution returns `None`.
33. `StatementImportProvider` — ties 28–32 into `Provider`: directory
    scan, skip-already-processed by content hash, pick most-recent
    unprocessed file, extract+parse+ledger update, fall back to
    last-known balance.
    Tests: first-run processes the one file present; second `fetch()`
    with no new file returns the same last-known balance without
    re-parsing; a genuinely new file gets processed and updates the
    ledger; missing/nonexistent `watch_dir` and unknown institution map
    to `ProviderError::Other`.
34. Register `statement_import` in the CLI's provider registry
    (`main.rs`, alongside `maybe_register_plaid` — unconditional, no
    env-var gate). Wiring only, verified manually against a real dropbox
    directory end-to-end (same carve-out as `maybe_register_plaid`
    itself, which also has no dedicated unit test).

Follow-on: extending Phase J's `StatementParser` pattern to the
remaining §7 institutions. Built against real statement structure (field
labels/section headers only, no real balances/account numbers/names —
see D28's addendum below). **Correction**: this originally claimed
Morgan Stanley's row turned out to be covered by a Fidelity NetBenefits
statement, so no separate parser would be needed — wrong. Morgan
Stanley (via E*TRADE, acquired 2020) is a real, separate account; see
Phase N / D32.

35. `VanguardStatementParser` — handles both of Vanguard's statement
    sub-layouts (529/Savings-style and Cash Plus/Brokerage-style) behind
    one parser, since both share the same `"Account overview"` heading
    convention.
    Tests: string-literal fixtures (synthetic values only) — one
    matching account per layout, multi-account disambiguation via
    `account_hint`, the Cash Plus layout's `"Statement overview"`
    top-level total correctly ignored, no matching account, ambiguous
    match, unrecognized layout, missing as-of-date doesn't block a valid
    balance.
36. `FidelityStatementParser` (NetBenefits "Statement Details" layout)
    — `"Ending Balance $X"` as the canonical balance marker (distinct
    from the also-present `"Beginning Balance"`/`"Vested Balance"`
    lines), `account_hint` matched as a case-insensitive substring
    against the `"[Employer] 401(k) Plan"` heading since this layout has
    no account number at all.
    Tests: string-literal fixtures (synthetic values only) — single
    plan, multi-plan disambiguation via `account_hint`, no matching
    plan, ambiguous match, unrecognized layout, missing statement period
    doesn't block a valid balance.
37. Sources screen support for `statement_import` (closes the FR5 gap:
    tasks 28–36 built the provider but left no way to actually add one
    through the UI). Adds `statement_import` to `PROVIDER_OPTIONS`, a
    required `watch_dir` field and optional `account_hint` field to the
    generic add/edit form, mirroring `webdriver`'s `login_url` pattern.
    Tests: `form.rs` — valid form has no errors, missing/blank
    `watch_dir` is an error, missing `account_hint` is not,
    `to_source_config` embeds both fields into `provider_config`
    correctly (including omitting `account_hint` when absent).

## Phase K — Statement auto-discovery (`~/Statements/<Institution>/<Account>`)

Decided post-v0.1 (D29): tasks 28–37 made statement import a fully
first-class provider, but every source still had to be added by hand,
one directory at a time. This phase adopts a fixed directory convention
and auto-creates sources from it, closing that gap the same way task 37
closed the "no way to add one" gap.

38. `ParsedStatement` gains a `category: Category` field, determined by
    the statement's own content rather than any directory-naming
    convention. `VanguardStatementParser`/`FidelityStatementParser`
    hardcode `Category::Asset` (neither institution has liability
    products in this app's scope, §7). `ChaseStatementParser` gets a new
    `detect_category` heuristic — generic, universal credit-card
    terminology (`"Minimum Payment Due"`, `"Credit Limit"`, `"Available
    Credit"`) → `Liability`, else `Asset`. Documented explicitly as an
    unverified heuristic, unlike the rest of this module's
    real-structure-verified parsing logic — only Chase's *checking*
    layout was ever confirmed against real statement wording.
    Tests: category assertions added to each parser's existing
    happy-path tests, plus dedicated Chase liability-keyword-detection
    cases (case-insensitive match included). Synthetic fixture text
    only, same standing rule as every other test in this module.
39. `discover_statement_sources(statements_root, existing_sources)` in
    new `crates/core/src/statement_import/discovery.rs` — walks
    `<root>/<Institution>/<Account>` two levels deep, skips leaves whose
    `watch_dir` is already registered, skips unrecognized institutions/
    empty leaves/unparseable statements (warned, not fatal — one bad
    leaf never blocks the rest of the scan), returns ready-to-add
    `SourceConfig`s with a generated `{institution}_{account}` id. Also
    applies a filename-based liability tiebreak: content stays the
    primary signal, but when it lands on the uncertain `Asset` default,
    a generic keyword check against the newest PDF's filename (`credit`,
    `card`, `visa`, `mastercard`, `amex`, `loan`, `mortgage`) can push it
    to `Liability` — never the reverse.
    Tests: happy path; already-known `watch_dir` not rediscovered;
    unrecognized institution skipped; empty leaf skipped; a leaf whose
    statement fails to parse skipped; multiple new leaves in one pass
    all discovered with distinct ids; missing root returns an empty
    list, not an error; a stray file directly under the root skipped; a
    third nesting level never walked into; a liability-hinting filename
    overrides an Asset default; a plain filename leaves it unaffected.
40. Wire into `crates/cli/src/main.rs`: new `statements_root()` (fixed
    `~/Statements`, mirrors `storage_dir()`'s D17 precedent), called
    once per process right after the first `load_or_init` and before
    `determine_mode` — each discovered source is `add_source`'d and
    pushed into the in-memory `sources` list. Wiring only, verified
    manually against a real `~/Statements` tree end-to-end (same
    carve-out as task 34's `register_statement_import`).
41. `docs/spec.md` D29 decision record + this Phase K entry.

## Phase L — Apple Card via statement import (D30)

Decided post-v0.1 (D30): FR2 originally planned `ManualEntryProvider`
(task 15) specifically for Apple Card, assuming no downloadable
statement existed. Corrected — the Wallet app exports a PDF statement —
so this closes FR2 the same way tasks 35–36 closed Vanguard/Fidelity,
via `StatementParser`, not a new provider mechanism. Supersedes task 15
for this institution; see task 15's note for why `ManualEntryProvider`
itself stays parked.

42. `AppleCardStatementParser` (Goldman Sachs Bank USA statement
    layout) — verified against a real Apple Card statement (field
    labels only, never a real balance/account number/name/email).
    Always `Category::Liability` (no asset variant, so no content-based
    detection needed, unlike Chase). No account number in this layout
    (same situation as Fidelity NetBenefits) — the `"Apple Card
    Customer"` line naming the cardholder is used as the raw
    `account_identifier` instead. `"Total Balance $X"` is the canonical
    marker; explicitly excludes any match immediately preceded by
    `"Previous "`, since `"Previous Total Balance $X"` (the prior
    period) would otherwise be silently matched instead, given
    `"Total Balance $"` is literally a substring of it.
    Tests: string-literal fixtures (synthetic values only) — parses the
    current (not previous) total balance, always categorized as
    liability, `account_hint` substring-matches the customer line,
    hint mismatch errors, unrecognized layout errors, missing as-of-date
    doesn't block a valid balance.
43. `parser_for` gains `"applecard"`/`"apple card"` match arms (both,
    since a user's `~/Statements/<Institution>/` folder name could
    reasonably be typed either way).
44. `docs/spec.md` FR2 + §7 table updates, D30 decision record, this
    Phase L entry.

## Phase M — Vanguard Brokerage holdings breakdown (D31)

Decided post-v0.1 (D31, FR22): every account so far reports exactly one
balance. This phase adds a second data dimension for one account type —
individual positions within an account, not just its total — so
concentration risk (e.g. too much of a portfolio in one stock) is
visible. Scoped to Vanguard Brokerage only; rendered as proportional
bars (ratatui has no native pie chart).

45. `Holding` type + `Asset.holdings`/`AccountRecord.holdings` +
    `Account::holdings()` default method (test-first). Tests: `Asset`
    with/without holdings; trait default returns `None` for `Liability`.
46. `AccountRecord` serde round-trip with `holdings` (mirrors
    `error_message`'s existing tests) + an old-snapshot-file-with-no-
    `holdings`-key-still-loads regression test, matching
    `processed_files.rs`'s `last_processed_mtime_secs` `#[serde(default)]`
    precedent from the same session.
47. `engine.rs` wiring (`account_to_record`, `error_record`,
    `record_to_account`) + every other `Asset`/`Liability`/
    `AccountRecord` construction site updated to keep compiling
    (`plaid_provider.rs`, `pii.rs`'s `scrub()`, `provider.rs`/
    `networth.rs`/`storage.rs`/`engine.rs`'s test fakes).
48. `crates/core/src/holdings.rs` — `AssetClass` (`Cash`/`Fund`/`Stock`)
    + `classify()` + `bucket()`, test-first against synthetic `Holding`
    fixtures. Deliberately separate from parsing/schema — a pure,
    read-time aggregation, so reclassifying later never needs a new
    statement parse or a schema migration.
49. `ParsedStatement.holdings: Vec<Holding>` field + `statement_import/
    mod.rs`'s `fetch()` wiring — fresh-parse-only, never carried forward
    via the ledger. Tests: a parser that never populates holdings leaves
    `Asset.holdings` as `None`, not `Some(vec![])`; a no-new-statement
    run never carries holdings forward even though balance still does
    via `last_known()`.
50. `vanguard.rs` `"Sweep program"`/`"Mutual funds"` table extraction —
    verified against a real Cash Plus/Brokerage statement (field
    labels/section headers only, never a real balance/account number/
    name). Takes the *last* dollar amount per row (not the second —
    fund rows show three amounts: price, balance-on-date-1,
    balance-on-date-2), following the same earlier-date-first,
    current-date-last convention this statement uses elsewhere. Bounded
    so the "Mutual funds" table's row-scan never reads into the
    following "Account activity" section, and so each row (including
    the last one, which has no next-symbol boundary to stop at) is
    bounded by its own nearest following blank line rather than the
    next symbol or the section end — the latter was a real bug caught
    while writing this task's own tests (the last row would otherwise
    run into the table's own totals line).
    Tests: string-literal fixtures (synthetic values, real Vanguard
    fund tickers/names — public product names, not personal data) —
    sweep cash position extracted as the current-period amount; each
    mutual fund row extracted correctly; extraction never reads past
    "Account activity"; a 529/Savings-style statement (no holdings
    tables at all) produces no holdings, not an error; `parse()` threads
    combined sweep + fund holdings through into `ParsedStatement`.
51. `dashboard.rs`: `draw_holdings_breakdown()` — one proportional
    horizontal bar per asset-class bucket, conditionally shown only
    when the snapshot has at least one account with holdings. Manual
    TUI verification only, per this project's existing rendering-code
    carve-out (§5/D9).
52. `docs/spec.md` FR22 + D31 decision record, this Phase M entry.

## Phase N — Morgan Stanley / E*TRADE statement parser (D32)

Decided post-v0.1 (D32): **corrects a wrong earlier claim** (Phase J's
"Follow-on" note and D28's addendum both said Morgan Stanley's §7 row
turned out to be covered by a Fidelity NetBenefits statement, so no
separate parser was needed — this was wrong. Morgan Stanley acquired
E*TRADE in 2020; it's a real, separate brokerage account with its own
stock/RSU holdings, unrelated to Fidelity NetBenefits.

53. `MorganStanleyStatementParser` — verified against a real "Client
    Statement" (field labels/section headers only, never a real
    balance/account number/name). Balance from the `"BALANCE SHEET"`
    section's `"TOTAL VALUE $X $Y"` line (last of 2 amounts, same
    earlier-then-current convention as every other parser). Account
    identifier from the account-number line following the account-type
    heading — verified against exactly one real account-type label
    (`"Morgan Stanley at Work Self-Directed Account"`); other Morgan
    Stanley/E*TRADE account types may use a different label, a stated
    limitation, not something guessed past. Always `Category::Asset`
    (matches Vanguard's same no-liability-products simplification).
    Tests: string-literal fixtures (synthetic values, real product/
    section-header text) — parses current (not prior) period total
    value; account_hint exact-matches the account number; hint mismatch
    errors; unrecognized layout errors.
54. Morgan Stanley holdings extraction — cash/BDP/MMF summary line (a
    single dollar amount) plus individual `"COMMON STOCKS"` rows.
    Stock rows are structurally different from Vanguard's: **Market
    Value is the third dollar amount in the row, not the last** (a row
    shows Share Price, Total Cost, Market Value, Unrealized Gain/Loss,
    Est Ann Income, in that order — five amounts, verified against
    exactly one real holding with all five columns populated; a
    position with missing cost-basis data, shown as `"—"` per this
    statement's own disclosures, could shift this counting — a stated
    limitation). Deliberately excludes `"STOCK PLAN DETAILS"` (unvested/
    potential RSU shares) — this statement's own disclosure states
    those values aren't actual held assets and aren't SIPC-protected.
    Tests: string-literal fixtures — cash position extracted; a stock
    row's Market Value extracted correctly (explicitly not Share Price
    or Total Cost — the point of this test); extraction never reads
    past `"ALLOCATION OF ASSETS"`; RSU rows in `"STOCK PLAN DETAILS"`
    are never counted as holdings.
55. `parser_for` gains `"morganstanley"`/`"morgan stanley"`/`"etrade"`/
    `"e*trade"` match arms (both institution names, since Morgan
    Stanley acquired E*TRADE and both names appear on real statements).
56. Correct the wrong "Morgan Stanley covered by Fidelity" claim in
    this file (Phase J's "Follow-on" note) and `docs/spec.md` (D28's
    addendum) + `docs/spec.md` D32 decision record + this Phase N
    entry.

## Phase O — Chase Checking real-statement fixes (D33/D34)

A real Chase Checking statement surfaced two unrelated gaps: `pdf-extract`
can't handle this statement's `SymbolEncoding` fonts at all (not a
version-bump fix — confirmed against the crate's own unreleased source),
and `chase.rs`'s existing checking/credit-card layouts didn't cover this
statement's actual structure.

57. `pdftotext` (poppler-utils) fallback in `pdf_text::extract_text` —
    tried whenever `pdf-extract` fails, by panic or by `Err`. Not a
    Cargo dependency; `None` (not an error) if the binary is missing or
    fails, so the caller still reports `pdf-extract`'s own error in that
    case. Not covered by an automated test (would require bundling or
    asserting on a real `pdftotext` install in CI) — verified manually
    against the real statement that surfaced the gap, same carve-out as
    this module's terminal setup/teardown (§5/D9).
58. `chase.rs` gains a third real layout: `"Account Number:"` followed
    by the full, unmasked account number (not a masked last-4 group) —
    account-id extraction now always keeps the last 4 digits of
    whatever digit run it finds. Balance extraction falls back to
    finding `"Ending Balance"` and then the next `"$"` within a bounded
    window when the adjacent `"Balance $"` substring isn't found (this
    statement's `"CHECKING SUMMARY"` table splits each row's label and
    dollar value onto separate lines, a pdf-extract column-order
    artifact). `extract_account_sections` now dedupes by last4 — this
    statement repeats `"Account Number:"` on every page, which a
    single-page synthetic fixture never exercised, and without deduping
    was misread as multiple accounts and rejected as `AmbiguousMatch`.
    Tests: full-unmasked-number reduces to its last 4 digits; a split
    label/value "Ending Balance" is still found; the same account
    repeated across pages is not treated as ambiguous.
59. `docs/spec.md` D33 + D34 decision records, this Phase O entry.
60. Verify build/tests, commit, manual end-to-end walkthrough against
    the real Chase Checking statement via `~/Statements/Chase/Checking`.

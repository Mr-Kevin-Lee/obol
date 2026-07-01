# Financial Health Dashboard — Specification

Status: Ready for implementation
Purpose: Input to design and implementation phases (spec-driven development)

**Starting point for implementation:** begin with the v0.1 scope in §15 —
core library + CLI/TUI. Part 1 (§1–3) is the functional requirements and
should read as the "what" and "why." Part 2 (§4–16) is the "how" — security
design, architecture, tech stack, and phasing. **Development is test-first
for all core-library logic (§5, D9)** — write the failing test before the
implementation. Don't build the HTTP interface (§6.1, §14, D7) until v0.1/v0.2
are complete and its security design has actually been done — see D7's
explicit deferral.

**v0.1 broken into commit-sized tasks:** see [tasks.md](tasks.md).

---

# Part 1 — Functional Requirements

## 1. Overview

A locally-run macOS dashboard that snapshots balances across checking, brokerage,
retirement, and credit accounts, then displays current net worth and per-account
detail. Complements Quicken Simplifi — Simplifi owns transaction-level budgeting,
this tool owns "what is my financial health right now."

**Primary user:** single user, run on-demand (later: on a schedule).

## 2. Functional requirements

### 2.1 Data sources & accounts

- **FR1** — Fetch balances from: Chase (checking, credit card), Vanguard
  (brokerage, 529, money market), Fidelity (401k), and Morgan Stanley/E-Trade
  (stocks/RSUs). See §7 for the connection approach per institution.
- **FR2** — Apple Card (Goldman Sachs) balance is entered manually each run,
  since it has no reachable web portal for either an aggregator or browser
  automation. See §7.
- **FR3 (stretch)** — Student loan and mortgage balances, once servicers are
  chosen.
- **FR4** — Account data is organized into two categories, Asset and
  Liability (the original "Credit/Debit" phrasing was clarified to this —
  see §11 and decision D1).
- **FR5** — Adding, editing, and removing a data source must be possible
  entirely through the UI (CLI/TUI now, HTTP interface later) — never by
  hand-editing a config file. See §10.1.

### 2.2 Net worth & display

- **FR6** — The dashboard calculates and displays current net worth (assets
  minus liabilities). See §12.
- **FR7 (stretch)** — Net worth broken down by asset type (cash, retirement,
  brokerage, 529, RSU/stock), presented as a pie chart. See §12.
- **FR8** — Account data is displayed in an easy-to-read format, with assets
  and liabilities visually grouped separately. See §13.
- **FR9** — The UI is colorblind-friendly — status and category are never
  conveyed by color alone. See §13.
- **FR10 (stretch)** — Identify trends in savings/spending, once at least
  two historical snapshots exist. See §13.

### 2.3 Snapshots & history

- **FR11** — Every run creates a snapshot of each connected account.
  Long-term, this runs on a schedule (e.g. biweekly). See §15 (v0.4).
- **FR12** — Snapshots contain no personally identifiable information. See
  §11.1.
- **FR13** — Snapshots are stored as versioned JSON. See §11.2.
- **FR14** — Historical snapshots must still render correctly after the
  app's own logic changes — backward compatibility is a hard requirement,
  not best-effort. See §11.3.

### 2.4 Credentials, security & privacy

- **FR15** — The app prompts for credentials as needed but never persists
  raw bank passwords to disk. See §8.
- **FR16** — Security and privacy take priority over convenience wherever
  the two are in tension. This isn't a single checklist item — it shapes
  every other requirement below it, and is elaborated as its own design
  section rather than scattered notes. See §4.

### 2.5 Reliability

- **FR17** — Basic retry logic on every data-source query, to absorb
  transient issues like timeouts. See §9.
- **FR18** — If a data source is unavailable, its panel clearly indicates
  failure while every other panel continues to render normally. See §9.

### 2.6 Interfaces

- **FR19** — Usable as a CLI/TUI (built first) and, later, as a local
  HTTP(S) endpoint reachable via browser — both retained long-term, not one
  replacing the other. See §6.1.
- **FR20** — The CLI/TUI must be usable for scheduled, headless runs
  without requiring a browser to be open. See §6.1.

### 2.7 Plaid usage visibility

- **FR21** — Both interfaces must surface how many of the 10 free-tier
  Plaid Items have been used, since that budget is limited and lifetime.
  See §7.1.

## 3. Non-goals (v1)

- Not a transaction-level budgeting tool (Simplifi's job).
- Not a multi-user or cloud-hosted product.
- Not a place raw bank credentials (passwords) are ever persisted to disk —
  see §4 for the one narrow, documented exception.
- No investment advice, forecasting, or tax computation.

---

# Part 2 — Technical Requirements & Design

## 4. Security & privacy design

This tool sits directly in front of live banking credentials and full account
balances, so security is treated as a first-class requirement throughout the
spec, not a follow-on concern bolted onto the end. This section is the
technical design behind FR16 (§2.4).

**Threat model — in scope:**
- Other processes or users on the same machine reading snapshot files,
  config, logs, or crash reports.
- Accidental leakage of credentials or balances into logs, error messages,
  clipboard history, or shell history.
- A compromised or malicious third-party dependency exfiltrating data.
- Snapshot data leaving the device unintentionally via a cloud-synced folder.

**Out of scope** (for a personal, single-machine tool): a root-level attacker
already on the machine, physical device compromise, and nation-state-grade
adversaries. Standard macOS protections (FileVault, Gatekeeper, the account
password) are assumed to be the outer perimeter — worth confirming FileVault
is enabled on this machine as a baseline.

**Secrets & credentials:**
- No bank password is ever written to disk, an environment variable, a log
  line, or a crash report, at any point, regardless of which provider is
  active (§8, §10).
- The one exception — the Plaid access token — is stored in macOS Keychain
  with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` (never synced to iCloud
  Keychain, inaccessible while the device is locked), scoped to this app's
  own access group so no other application can read it.
- **Language choice is driven by this requirement (decision D6, §16):**
  secrets are wrapped in `secrecy` types (preventing accidental exposure via
  `Debug`/logging) and zeroized on drop via `zeroize`, giving a deterministic,
  compiler-enforced cleanup point instead of Python's GC-timed collection.
- **Honest caveat, even in Rust:** `zeroize` shrinks the exposure window, it
  doesn't eliminate it. It doesn't stop a debugger attached via `ptrace`, and
  doesn't by itself prevent a secret from being paged to disk — that would
  need `mlock` as well, which adds complexity for a marginal gain on a
  single-user laptop. The spec should describe this as meaningfully reduced
  risk, not an absolute guarantee.
- No plaintext logging of credentials, full account numbers, or raw
  provider-API responses, at any log level, ever.

**Data minimization & storage:**
- PII scrubbing (§11.1) is the first line of defense — snapshots contain no
  account numbers or names by design, so a leaked snapshot file is far less
  damaging than a leaked bank statement would be.
- Snapshot and config files are written with restrictive permissions
  (`0600` files, `0700` directory) — never group- or world-readable.
- Default storage location is deliberately **not** `~/Documents`,
  `~/Desktop`, or any path inside an iCloud Drive/Dropbox/OneDrive sync root
  (use e.g. `~/Library/Application Support/FinancialDashboard/`) — scrubbed
  data shouldn't silently propagate to cloud storage. Backing up snapshots to
  the cloud, if ever wanted, should be a separate, explicit opt-in.
- Pseudonymous account keys (§11.2) are **salted** hashes, not raw
  institution identifiers, so they can't be reversed to the real account
  number even if a snapshot file leaks.

**Network & process isolation:**
- All outbound calls (Plaid, WebDriver-driven bank sites) use TLS with
  certificate verification enabled — never disabled for convenience, even in
  development.
- Any local server component binds to `127.0.0.1` only, never `0.0.0.0` —
  the dashboard should never be reachable from the local network, including
  on shared Wi-Fi. This applies directly to the v0.3 local HTTP interface
  (§16, D7).
- Browser automation uses a fresh, isolated browser context per run rather
  than a persistent profile, so bank session cookies don't accumulate on
  disk between runs.

**Supply chain:**
- Dependencies pinned via `Cargo.lock` (committed to version control), not
  loose version ranges, so builds are reproducible and a compromised
  dependency update can't silently slip in.
- Dependency footprint kept intentionally small — each addition is a new
  vector for supply-chain risk in a tool that handles bank credentials.
- No telemetry, analytics, or crash reporting phones home by default. If
  ever added, it must be explicitly opt-in, clearly disclosed, and must
  never include balances or account identifiers.

**Auditability:**
- Each run logs a minimal local history (timestamp, which sources
  succeeded/failed — no balances, no identifiers) so connection health is
  visible over time without the log itself becoming a sensitive artifact.

## 5. Development methodology: test-first

For all **core library logic**, the failing test is written before the
implementation that makes it pass. This isn't a blanket policy applied
uniformly everywhere — different layers of the system warrant different
rigor, and the spec is explicit about which is which:

**Test-first, no exceptions** (deterministic, no live network dependency):
- `Provider` trait contract — tested against fake/in-memory providers, not
  real institutions, so this suite runs instantly and offline.
- Retry/backoff behavior (§9) — attempt count, backoff timing, jitter
  bounds, and that auth failures fail fast without retrying.
- PII scrubbing (§11.1) — explicit tests asserting that no account number,
  name, or raw provider response ever appears in a serialized snapshot.
- Snapshot JSON schema (§11.2) serialization/deserialization, and the
  backward-compatibility migration chain (§11.3) — old-version fixtures must
  still load correctly after every schema change.
- Net worth calculation (§12) — asset/liability summation, exclusion of
  failed sources from the total.
- Plaid Item usage counter (§7.1) — increments on creation, never
  decrements on removal, blocks new connections at 10/10.
- Sources config CRUD (§10.1) — add/edit/remove against `sources.yaml`,
  including the atomic-write behavior.

**Integration-tested, not unit-tested** (real external systems, inherently
non-deterministic — a separate, explicitly lighter-weight tier, not the TDD
loop):
- The real Plaid client against Plaid's **Sandbox** environment (§7) — this
  is where "does the HTTP request/response actually work" gets verified,
  distinct from the unit-tested `Provider` trait contract above.
- `fantoccini`/WebDriver behavior against a real bank-style login flow —
  this is exactly what the v0.1 spike (§15) exists to validate; it's a
  go/no-go check, not something to force into a TDD red-green loop against
  a live third-party site.

**Tooling:** `cargo test` with the standard `#[test]` harness; trait-based
fakes for `Provider` (hand-rolled or via `mockall`) rather than mocking
Plaid/WebDriver directly; `proptest` is worth using for the retry/backoff
jitter bounds and the schema migration chain, where "for all valid inputs,
this invariant holds" is a more natural fit than enumerated example cases.
**Naming note:** the `insta` crate (snapshot testing — captures a value and
diffs it against a saved baseline) is a good fit for the JSON schema tests
in §11.2, but its "snapshot" is a different concept from the app's own
balance snapshots — worth keeping that distinction clear in test names and
comments so the two don't get confused.

## 6. Architecture

```
[Plaid connector]      [Browser automation connector]
        \\                     /
         \\                   /
          v                 v
         Snapshot engine (retry + PII scrub)
                    |
                    v
         JSON snapshot store (versioned, local disk)
                    |
                    v
      CLI/TUI  <——— core library ———>  Local HTTP interface (browser)
```

Every data source is implemented behind a common `Provider` interface (see
§10), so the snapshot engine, retry logic, and dashboard never need to know
which institution — or which underlying aggregator — they're talking to.
Swapping Plaid for a different aggregator, or moving a single institution to
browser automation, is a config + one-provider-class change, not a rewrite.

### 6.1 Two interfaces, one core

Everything above the `Provider` layer — snapshot engine, storage, PII
scrubbing, net worth calculation — lives in a single core library with no UI
code in it. Two separate front ends call into that same core:

```
                 core library
      (providers, snapshot engine, storage,
         net worth calc — no UI code)
              /                    \
             /                      \
    CLI / TUI              Local HTTP interface
  (terminal, scriptable)   (browser, 127.0.0.1)
```

This isn't just a build-order convenience — both interfaces are meant to
stay around long-term, for different purposes (FR19, FR20, §2.6):

- **CLI/TUI** — the right tool for scheduled, headless runs (a `launchd` job
  running a biweekly snapshot shouldn't need a browser open), and for
  scripting/debugging during development.
- **Local HTTP interface** — the right tool for day-to-day interactive
  viewing, reached at `127.0.0.1:<port>` in your regular browser. This
  becomes the default way you'll actually use the dashboard once it exists.

Because both sit on the same core, nothing built for the CLI/TUI phase is
throwaway work — the HTTP interface phase adds a second front end, not a
rewrite. See §15 for the phased build order and §14 for the specific
libraries behind each interface.

### 6.2 Data flow (the default run)

`main.rs` is a thin `clap` command dispatcher — different subcommands
compose the same core calls differently, so the flow below isn't one
linear function but the body shared by the interactive default command and
(mostly) `obol snapshot`:

- **`obol` (default, interactive TUI):** load sources → fetch → save → render.
- **`obol snapshot` (headless, for `launchd`/FR20):** load sources → fetch
  → save. No render step — logs the per-source outcome (§4 auditability)
  and exits.
- **`obol sources`:** jumps straight to the Sources screen (§10.1) — no
  fetch at all.

**First run branches before any of this.** If `sources.yaml` is missing or
empty, core creates an empty one (§10.1) and the CLI opens directly to the
Sources screen with a prompt to add a source — it does not run a snapshot
against an empty source list and render a blank dashboard.

**Steady-state flow, core calls in order:**

1. `core::sources::load_or_init()` — reads `sources.yaml`, creating it if
   absent (§10.1).
2. `core::snapshot::run(sources, &dyn CredentialSource)` — the snapshot
   engine (§6 diagram). Internally:
   - Providers are instantiated once per **provider type**, not per
     source (so e.g. all Plaid sources share one HTTP client) — looked up
     from the `provider_registry` (§10).
   - Per source, concurrently (sources are independent I/O — §9's
     per-source isolation extends naturally to concurrent fetch, and
     §7.1's rate limits are nowhere near a concern at this volume):
     - Resolve credentials: Plaid sources pull the stored access token
       from Keychain directly (no prompt, §8); `webdriver` and
       `manual_entry` sources go through the `CredentialSource` callback
       (below).
     - Call `provider.fetch()` wrapped in the `RetryIf`-based retry
       policy (§9, D10).
   - Assemble results into a `Snapshot`: PII-scrub (§11.1), build one
     `AccountEntry` per source with `status: "ok"` or `"error"` (§9's
     per-source isolation — one failure never aborts the run).
3. `core::storage::save_snapshot(&snapshot)` — atomic write, `0600`
   (§11.2).
4. `core::storage::load_recent_snapshots(n)` — independent of steps 2–3;
   feeds the Sources screen's "last updated N runs ago" and the stretch
   trend chart (§13), not the net-worth figure itself.
5. `core::networth::calculate(&snapshot)` — pure function, depends
   **only** on the fresh snapshot from step 2, not on step 4's history.
6. **CLI-only, not core:** render (`ratatui`) using the snapshot, net
   worth summary, recent history, and the Plaid Item usage counter (§7.1).

Steps 3 and 4 have no data dependency on each other and can run
concurrently; step 5 depends only on step 2's output, not step 4's.

**The `CredentialSource` callback (decision D12, §16).** §6.1 requires
core to contain no UI code, but §8 requires interactive prompting for
webdriver credentials and the manual-entry balance on every run — both
needed *before* `provider.fetch()` can be called, since `fetch()` takes
`credentials: Option<&Credentials>` rather than prompting internally. The
core snapshot engine takes a trait object rather than importing a UI
crate:

```rust
trait CredentialSource {
    /// Called once per source that needs interactive input this run
    /// (webdriver credentials, or the manual-entry balance). Never called
    /// for Plaid sources — those resolve from Keychain internally.
    fn provide(&self, source: &SourceConfig) -> Option<Credentials>;
}
```

The CLI implements this with a masked terminal prompt; the future HTTP
interface (§14, D7) implements it as a form post. This is what lets both
front ends share `core::snapshot::run()` unchanged, consistent with
§6.1's "same core, two interfaces."

## 7. Data source strategy

Per your direction: **aggregator where a free option exists, browser automation
otherwise.**

| Institution | Account(s) | Recommended path | Notes |
|---|---|---|---|
| Chase | Checking, credit card | Plaid (free Trial tier) | Well-supported OAuth institution |
| Vanguard | Brokerage, 529, money market | Plaid | Investments product; verify 529 sub-account support during spike |
| Fidelity | 401k | Plaid | Retirement accounts generally supported |
| Morgan Stanley / E-Trade | Stocks, RSUs | Plaid | Confirm during spike — post-acquisition institution coverage varies |
| Apple Card (Goldman Sachs) | Credit card | **Manual entry**, fallback browser automation | No web portal — lives only in iPhone Wallet. Neither Plaid nor WebDriver automation can reach it. Recommend a simple "enter balance" panel for this one source. |
| Student loans (stretch) | TBD | Browser automation | Servicer TBD; revisit once selected |
| Mortgage (stretch) | TBD | Browser automation | Servicer TBD |

**Why Plaid first:** as of April 2026, Plaid offers a free Trial plan for new
US/Canada teams supporting up to 10 live Production accounts with real data —
comfortably covers your 5 non-Apple-Card institutions. No cost, no per-call
billing, until you exceed 10 linked accounts.

**What "Item" actually means:** a Plaid Item is one login at one institution,
not one account. If Vanguard exposes brokerage + 529 + money market under a
single sign-in, that's one Item. Estimated Item usage for this project:

| Institution | Accounts | Items |
|---|---|---|
| Chase | Checking + credit card | 1 (assuming shared login) |
| Vanguard | Brokerage + 529 + money market | 1 (assuming shared login) |
| Fidelity | 401k | 1 |
| Morgan Stanley/E-Trade | Stocks/RSUs | 1 |
| **Total** | | **~4 of 10**, leaving room for stretch goals |

Balance, Investments, and Liabilities — the specific products this project
needs — are all included free under the Trial plan (Investments and
Liabilities are normally subscription-billed per Item; that's waived on
Trial).

**Two implementation gotchas to design around:**

- **The 10-Item cap is lifetime, not concurrent.** Removing an Item via
  `/item/remove` does not free a slot. Relinking the same institution
  repeatedly while debugging burns Items permanently. **All development and
  integration testing should happen against Plaid's Sandbox environment**
  (fake institutions, unlimited free relinking); only link real institutions
  in Production once the connector code is stable, to conserve the budget.
- **Fidelity may require additional approval.** Plaid's docs flag Fidelity
  (and Charles Schwab) as institutions that can need an extra access request
  via the Compliance Center. Verify this during the spike rather than
  assuming it works out of the box.

**Access token persistence:** resolved in decision D2 (§16) — Plaid access
tokens are stored in macOS Keychain, scoped and revocable independent of any
real bank credentials. See §16 for the reasoning.

### 7.1 Item usage tracking & API rate limits

**Item usage must be surfaced in both interfaces (FR21, §2.7; decision D8,
§16).** Plaid does not expose an API to query how many of your 10 Trial
Items you've used — there's no "remaining quota" endpoint. That number has
to be tracked by the app itself, and it has to be a **lifetime counter**,
not a count of currently-configured Plaid sources, since `/item/remove`
doesn't free the cap (§7).

- A `plaid_items_created_lifetime` counter lives in local app state
  (alongside `sources.yaml`, same `0600` file protection as §4 — not a
  secret, but still worth protecting from accidental corruption).
- Incremented exactly once, at the moment a new Plaid Item is successfully
  created (public token exchanged for an access token) — during the Sources
  screen's "Connect via Plaid" flow (§10.1). Never decremented, including
  when a source is later removed.
- **Both the CLI/TUI and the (future) HTTP interface display this
  prominently** — e.g. "Plaid Items: 4/10 used" in the TUI header and on the
  Sources screen — not buried in a details view.
- Warn clearly at 8/10; block the "Connect via Plaid" flow entirely at
  10/10 with a message pointing at alternatives (a `webdriver` or
  `manual_entry` source instead, or upgrading off Trial).
- Because this number can't be reconstructed from Plaid if local state is
  ever lost, it's worth an occasional manual cross-check against the count
  shown on Plaid's own Dashboard (dashboard.plaid.com) — there's no
  programmatic way to self-heal this counter if it drifts.

**API rate limits (Production, from Plaid's docs) — not a practical
constraint for this project, but worth knowing:**

| Endpoint | Per-Item limit | Per-client limit |
|---|---|---|
| `/accounts/balance/get` (primary one we'll use) | 5/min, 30/hour | 1,200/min |
| `/accounts/get` | 15/min | 15,000/min |
| `/auth/get` | 15/min | 12,000/min |
| `/identity/get` | 15/min | 2,000/min |

A biweekly run making one balance call per Item is nowhere near these. The
one place this is worth designing around: **repeated manual runs while
debugging in the same hour** could approach the 30/hour-per-Item cap on
`/accounts/balance/get`. The retry policy (§9, 3 attempts with exponential
backoff) doesn't meaningfully contribute to this on its own — but if the TUI
detects several consecutive runs against the same Item within a short
window, it should surface a soft warning rather than silently eating into
that budget. Sandbox has no such concern (100/min per Item), reinforcing the
existing guidance to do iterative testing there (§7).

## 8. Credential handling

- Every run prompts for credentials needed by that run's active sources
  (masked input, works identically in CLI/TUI and the HTTP interface) —
  except Plaid sources, which use a stored access token instead (decision
  D2, §16).
- **Implementation:** this prompting is threaded through the core snapshot
  engine via the `CredentialSource` trait (§6.2, decision D12) rather than
  core importing a UI crate directly — one callback interface, implemented
  differently by each front end.
- Non-Plaid credentials (browser automation passwords) are wrapped in
  `secrecy::Secret` immediately on input, live only for the duration of that
  source's fetch call, and are zeroized on drop via `zeroize` — see §4 for
  what this guarantees and what it doesn't. No writes to disk, environment
  variables, or logs.
- Plaid access tokens are stored in macOS Keychain (via the `security-framework`
  crate) under a single, documented, narrowly-scoped entry
  (`kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, own access group, not
  iCloud-synced) — inspectable and revocable independent of any real bank
  password, and independent of any other provider's credentials.
- Credential shape is provider-defined (see §10), so this asymmetry is
  contained entirely inside the Plaid provider implementation; the rest of
  the system treats "credentials" as an opaque, provider-specific value.

## 9. Retry & failure handling

- Each connector call wrapped in retry logic: **3 attempts, exponential
  backoff starting at 2s, doubling, ±20% jitter**, hard timeout of 15s per
  attempt.
- Retries apply to transient errors (timeouts, 5xx, connection resets) only —
  not to auth failures, which fail fast.
- **Implementation: `tokio-retry`'s `RetryIf`** (decision D10, §16) —
  `ExponentialBackoff::from_millis(2000).map(jitter).take(3)` for the
  strategy, with a `condition` closure that inspects the error and returns
  `false` for auth failures so they short-circuit instead of retrying. The
  per-attempt 15s hard timeout wraps each call via `tokio::time::timeout`,
  since `tokio-retry` itself doesn't impose one.
- **Per-source isolation:** a failed source produces a `status: "error"` entry
  in the snapshot with a human-readable message; it does not raise or abort
  the run. The dashboard renders every other panel normally and shows a clear
  "data unavailable" state for the failed one (not a blank or crashed panel).

### 9.1 Failure modes beyond per-source retry

The scenarios above cover one source failing transiently. The following are
distinct failure modes, worked through explicitly rather than left as
implicit gaps (decision D13, §16):

- **All sources fail in one run.** Net worth (§12) is only ever computed
  over `status: "ok"` accounts — if that set is empty, the dashboard does
  not render "$0" (indistinguishable from a genuine zero net worth). It
  shows an explicit failure state instead ("Net worth unavailable — 0/N
  sources returned data this run"), with every per-source panel still
  showing its own individual error.
- **Snapshot persistence is best-effort, not blocking.** If
  `core::storage::save_snapshot` fails (disk full, permissions, unexpected
  I/O error), the run does not abort — the CLI/TUI still renders the
  in-memory snapshot just fetched, but surfaces a clear warning that this
  run's data was not written to history (FR11's "every run creates a
  snapshot" didn't hold this time). The failure is logged (§4
  auditability) but never blocks the user from seeing what was just
  fetched.
- **A Plaid Keychain read failure is treated as a relink signal, not a
  generic error.** If the stored access token can't be read (locked
  Keychain, revoked/missing entry), that source's panel shows `status:
  "error"` with a message pointing directly at the Sources screen's
  existing "Reconnect" flow (§10.1's update-mode Link) — a missing or
  invalid token means the Item needs to be re-linked, not retried.
- **A `sources.yaml` that fails to parse** (YAML syntax error, structurally
  invalid) blocks that run entirely and says so plainly — "`sources.yaml`
  could not be parsed: `<underlying error>`" — rather than silently
  falling back to an empty source list or guessing at a partial parse. The
  whole file is unusable until it's fixed (from the Sources screen, or by
  hand, since it's still a plain YAML file on disk).
- **A syntactically valid entry referencing an unknown `provider:` name**
  (typo, or a provider not yet implemented) is a **per-source** failure,
  not a whole-run failure, consistent with this section's isolation
  principle — that one source gets `status: "error"` ("unknown provider:
  'x'"), every other valid source fetches normally.
- **WebDriver infrastructure failures are a distinct error category from a
  bad login**, and are not retried (retrying doesn't fix a missing
  `chromedriver`/`geckodriver` binary or a WebDriver session that failed
  to start) — same fail-fast treatment as auth failures, but with a
  diagnosable message distinct from "login failed" (e.g. "chromedriver not
  found on PATH" vs. "authentication failed").
- **Concurrent runs are serialized with an advisory file lock**, not left
  to race — a scheduled `launchd` run (§15, v0.4) overlapping with an
  interactive session could otherwise double-write `sources.yaml`, corrupt
  an in-progress atomic write, or double-increment the Plaid Item counter
  (§7.1). A single lock file (`.lock` in the app's storage directory, §4)
  is held for the duration of the write-critical sections — config writes,
  snapshot writes, Item counter increments — using an OS-level advisory
  lock (`flock`, via the `fs2` or `fslock` crate; a plain
  `std::sync::Mutex` doesn't help here since these are separate
  processes, not threads). A run that can't acquire the lock within a
  short timeout exits with a clear "another instance appears to be
  running" message rather than blocking indefinitely.

## 10. Connector architecture (source-agnostic, provider-swappable)

Two deliberately separate concepts, so that moving away from Plaid — in whole
or for a single institution — never touches the snapshot engine, retry logic,
storage layer, or dashboard:

- **Source** — a config-driven declaration of *one real-world account group*
  ("Chase checking + credit card"). Owns category, type, institution name,
  and which provider reaches it.
- **Provider** — the mechanics of *how* to reach a class of source: Plaid,
  WebDriver-based browser automation, manual entry, and (later, if desired)
  something like SimpleFIN or Yodlee. A provider knows nothing about any
  specific institution — it just implements the trait.

```rust
trait Provider {
    /// Returns balances for this source. Returns Err on failure — retry
    /// and error-capture happen in the snapshot engine, not here.
    fn fetch(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError>;
}

fn provider_registry() -> HashMap<&'static str, Box<dyn Fn() -> Box<dyn Provider>>> {
    let mut m: HashMap<&'static str, Box<dyn Fn() -> Box<dyn Provider>>> = HashMap::new();
    m.insert("plaid", Box::new(|| Box::new(PlaidProvider::new())));
    m.insert("webdriver", Box::new(|| Box::new(WebDriverProvider::new()))); // fantoccini-based
    m.insert("manual_entry", Box::new(|| Box::new(ManualEntryProvider::new())));
    // future: "simplefin" => SimpleFinProvider
    m
}
```

Sources are declared in `sources.yaml`, each pointing at a provider by name:

```yaml
sources:
  - id: chase_checking
    provider: plaid
    category: asset
    type: checking
    institution: chase
    account_salt: "b64:9fQxK2..."  # generated once at add-time, §11.1
    provider_config:
      plaid_institution_id: ins_56

  - id: vanguard_investments
    provider: plaid
    category: asset
    type: brokerage
    institution: vanguard
    account_salt: "b64:71pLm8..."
    provider_config:
      plaid_institution_id: ins_12

  - id: apple_card
    provider: manual_entry
    category: liability
    type: credit_card
    institution: goldman_sachs
    account_salt: "b64:3dRzT0..."

  - id: student_loan_navient
    provider: webdriver
    category: liability
    type: student_loan
    institution: navient
    account_salt: "b64:eV9wCq..."
    provider_config:
      login_url: "https://navient.com/login"
```

**Credential shape** is provider-defined (see above), so this asymmetry is
contained entirely inside the Plaid provider implementation; the rest of the
system treats "credentials" as an opaque, provider-specific value.

**What this buys you:**

- **Removing a source** = deleting its entry in `sources.yaml`.
- **Adding a source** = new entry, referencing an existing provider if one
  fits.
- **Moving one institution off Plaid** (e.g. Vanguard support gets flaky) =
  write a new provider or point that source at `webdriver`, change one
  `provider:` field. Every other source is untouched.
- **Moving off Plaid entirely** (e.g. cost, ToS changes, or the Trial Item
  budget runs out) = implement one new `Provider` (say, `SimpleFinProvider`)
  and bulk-update the `provider:` field across `sources.yaml`. The snapshot
  engine, PII scrubbing, retry wrapper, JSON schema, and dashboard require
  **zero code changes** — they only ever see the `Provider` trait and
  `AccountBalance` output, never a Plaid-specific type.
- Credential shape varies by provider (Plaid: a stored access token; browser
  automation: username/password prompted each run; manual entry: none) — the
  `credentials` parameter is provider-defined, so this variance stays fully
  contained inside each provider implementation.

### 10.1 Managing sources through the UI

`sources.yaml` remains the underlying file on disk (git-friendly, inspectable,
scriptable) — but it's no longer hand-edited. A **Sources** screen in the
dashboard reads and writes it directly, so add/edit/remove never requires
opening a text editor (FR5, §2.1).

**First run:** if `sources.yaml` doesn't exist yet, the TUI creates an empty
one (`sources: []`, `0600`, in the storage path from §4) rather than
erroring, and opens directly to the Sources screen with a short prompt to
add the first source — not an empty net worth dashboard showing nothing.

**Provider-described config schemas.** Each `Provider` declares its own
config shape as a `serde`-deserializable struct:

```rust
#[derive(Deserialize, Serialize)]
struct PlaidProviderConfig {
    plaid_institution_id: String,
}

#[derive(Deserialize, Serialize)]
struct WebDriverProviderConfig {
    login_url: String,
}

#[derive(Deserialize, Serialize)]
struct ManualEntryProviderConfig {} // no extra fields needed
```

The "add/edit source" form is **generated from whichever schema the selected
provider declares** — pick a provider from a dropdown, the relevant fields
appear. This is what keeps the UI itself provider-agnostic: adding a new
provider later (e.g. SimpleFIN) means writing its config model, and the UI
gains a working form for it with no UI code changes.

**Add flow, generic case** (manual entry, browser automation): fill in id,
display name, category, type, institution, provider-specific fields →
generate a random `account_salt` for this source (§11.1, D15) → write new
entry to `sources.yaml` (atomic write — temp file + rename, so a crash
mid-write can't corrupt the config).

**Add flow, Plaid case** — needs an interactive auth step, not just form
fields, since Plaid Link is a hosted authentication flow:
1. "Connect via Plaid" button calls `/link/token/create` configured for
   Hosted Link.
2. Dashboard displays the returned URL (opens in system browser, or as a QR
   code for completing on your phone).
3. Dashboard polls `/link/token/get` (results available for 6 hours post-
   session) until the session completes.
4. On success, exchanges the `public_token` for an `access_token`, stores it
   in Keychain, **increments `plaid_items_created_lifetime` (§7.1)**, and
   writes the new source entry, including a freshly generated
   `account_salt` (§11.1) — no separate webhook server needed for a
   single-user local tool.

**Why up to 6 hours (decision D18, §16):** that's Plaid's own guarantee
window for how long a completed Link session's result stays retrievable
via `/link/token/get` — not a typical completion time. Most sessions
complete in minutes, but the Hosted Link flow is explicitly built for
"scan the QR code on your phone," so a user could plausibly start it, get
distracted, and finish it later. Polling therefore runs as a **background
async task** (`tokio::spawn`, polling on an interval, not a blocking loop)
so the TUI stays responsive and the user can navigate away from the
Sources screen while a Link session is pending, rather than the whole
interface freezing until it completes or times out. The pending session is
cancelable from the Sources screen.

**Edit flow:** re-opens the same generated form pre-filled with current
values. For Plaid sources specifically, "Reconnect" re-runs Link in **update
mode** (Plaid's mechanism for repairing a broken Item or adding accounts)
rather than requiring a full re-auth.

**Remove flow — cleanup matters more than deleting a config line:**
- Manual entry / browser automation sources: delete the `sources.yaml` entry.
  Nothing external to clean up.
- **Plaid sources: call `/item/remove` before deleting the config entry.**
  This releases the Item and ends any subscription billing tied to it
  (Investments/Liabilities are subscription-billed products — see §7).
  Skipping this step leaves a dangling Item that still counts against the
  10-Item Trial budget and could incur charges after upgrading off Trial.
  The UI should make this an explicit, confirmed action ("Remove and
  disconnect from Plaid") rather than a silent side effect.
- The corresponding Keychain entry is deleted in the same step.

## 11. Data model

**Terminology note (decision D1):** the original requirements used "Credit"
and "Debit" — read as **Asset** (checking, brokerage, 401k, RSUs) vs.
**Liability** (credit card balances, mortgage, student loans), the standard
net-worth framing (FR4, §2.1).

Account types (extensible list, not a hardcoded enum in the schema — new types
shouldn't require a schema version bump):

- Assets: `checking`, `money_market`, `brokerage`, `retirement_401k`, `529`, `rsu_stock`
- Liabilities: `credit_card`, `mortgage`, `student_loan`

**Shared `Account` trait (decision D11, §16).** Mirroring the `Provider`
trait's role in §10, `Asset` and `Liability` implement one common `Account`
trait rather than being distinguished by scattered `if category == ...`
checks throughout net worth calc (§12) and dashboard rendering (§13):

```rust
trait Account {
    fn account_key(&self) -> &str;
    fn institution(&self) -> &str;
    fn balance(&self) -> Option<f64>;
    fn status(&self) -> &AccountStatus;
    /// Signed contribution to net worth — positive for assets, negative
    /// for liabilities. Centralizes the one place that sign logic lives.
    fn net_worth_contribution(&self) -> f64;
}

struct Asset { account_key: String, institution: String, r#type: String, balance: Option<f64>, status: AccountStatus }
struct Liability { account_key: String, institution: String, r#type: String, balance: Option<f64>, status: AccountStatus }
```

`Provider::fetch` (§10) returns `Vec<Box<dyn Account>>` instead of a bare
struct — a provider already knows a source's category from `SourceConfig`
(the `category:` field in `sources.yaml`), so it constructs an `Asset` or
`Liability` directly. Net worth calc (§12) and dashboard rendering (§13)
then only ever call `.net_worth_contribution()` / `.balance()` /
`.status()` through the trait, never branch on category by hand. This is a
runtime/domain-layer abstraction only — it doesn't change the on-disk JSON
schema (§11.2), which stays a flat `category` field per record; the storage
layer converts trait objects to/from that flat shape at the serialization
boundary.

**Balance representation (decision D16, §16):** kept as `f64` throughout —
the trait, the schema, and the summation in §12 — rather than a
decimal/cents-based type. This is a glance-at-it net worth dashboard, not
an accounting/reconciliation tool, so sub-cent float drift across a
handful of account balances is an accepted, deliberate tradeoff, not
deferred technical debt.

### 11.1 PII scrubbing rules

Snapshots store **no**: account numbers, account holder name, institution
login identifiers, or raw API response payloads. Snapshots store **only**:
institution name, a locally-generated, **salted** pseudonymous account key
(so the same account can be tracked run-over-run for trend analysis without
storing — or being reversible to — the real account number), category,
type, balance, currency, and timestamp.

**Salt storage (decision D15, §16):** the salt is generated once, per
source, at the moment that source is added (§10.1's add flow), and stored
alongside that source's entry in `sources.yaml` (`account_salt` field) —
not regenerated per run (which would break run-over-run tracking) and not
a single install-wide salt (which would let two leaked snapshots from
different installs be correlated). `sources.yaml` already carries the same
`0600` protection as snapshot files (§4), so this doesn't introduce a new
class of sensitive file — it's a config-adjacent value living where the
rest of that source's non-secret configuration already lives, a pragmatic
"for now" choice rather than a permanent one.

Storage hardening (see §4 for full rationale):
- Snapshot and config files written with `0600` permissions, containing
  directory `0700`.
- Default storage path is outside any cloud-synced folder (e.g.
  `~/Library/Application Support/FinancialDashboard/`), not `~/Documents`,
  `~/Desktop`, iCloud Drive, or Dropbox.
- **Storage path is fixed (not configurable) in v1** (decision D17, §16) —
  deferred rather than designed now, since an override mechanism
  immediately reopens the exact question this bullet exists to close
  (validating the override doesn't land in a cloud-synced folder).
  Configurability is a named v0.4 stretch item (§15).

### 11.2 Snapshot JSON schema

```json
{
  "schema_version": 1,
  "snapshot_id": "b3f1...(uuid)",
  "created_at": "2026-06-30T09:15:00-07:00",
  "accounts": [
    {
      "account_key": "sha256:9f2a...",
      "source_id": "chase_checking",
      "institution": "Chase",
      "category": "asset",
      "type": "checking",
      "balance": 4213.55,
      "currency": "USD",
      "status": "ok"
    },
    {
      "account_key": "sha256:71bd...",
      "source_id": "apple_card",
      "institution": "Goldman Sachs",
      "category": "liability",
      "type": "credit_card",
      "balance": null,
      "currency": "USD",
      "status": "error",
      "error_message": "Manual entry not provided for this run"
    }
  ]
}
```

Net worth is **computed at render time** from raw balances, not stored in the
snapshot — this lets the calculation logic evolve (e.g. how RSUs are valued)
without invalidating historical snapshots.

### 11.3 Backward compatibility

- `schema_version` is mandatory on every snapshot file.
- The dashboard's snapshot loader runs a migration chain
  (`migrate_v1_to_v2`, etc.) in memory when loading an older snapshot —
  stored files are never rewritten.
- New account types are additive and don't require a version bump; only
  breaking structural changes (field renames/removals) do.
- **Forward compatibility, not just backward (decision D14, §16).** A
  snapshot written by a **newer** version of the app (e.g. after
  reinstalling an older binary, or during a downgrade) is not rejected
  outright either. Deserialization relies on serde's default lenient
  behavior (no `#[serde(deny_unknown_fields)]`), so unrecognized fields
  and unrecognized account `type` values are ignored rather than causing a
  parse failure — the binary parses what it can and ignores what it
  can't, surfacing a visible-but-non-fatal note that some data from a
  newer schema version was skipped, rather than failing to load the
  snapshot at all.

## 12. Net worth & breakdown logic

- **Net worth** = sum(asset balances) − sum(liability balances), computed only
  over accounts with `status: "ok"` for that run, with a visible note of which
  sources were excluded due to failure. Implemented as a sum of
  `.net_worth_contribution()` (§11) across all `Account` trait objects — the
  asset/liability sign logic lives in exactly one place, not duplicated in
  the summation code.
- **All sources failed:** net worth is not shown as `$0` — see §9.1's
  explicit "unavailable" state for this case.
- **Stretch — asset breakdown pie chart:** group by `type` (cash, retirement,
  brokerage, 529, RSU/stock), rendered as a pie chart.

## 13. Dashboard UI

Two screens: the **net worth dashboard** (default view) and a **Sources**
screen for managing connections (§10.1).

**Dashboard:**
- One panel per source, each independently rendered — a failed source shows a
  clear "unavailable" state with the error message, not a blank space or a
  crashed page.
- Top-level net worth figure, prominent — replaced with an explicit "Net
  worth unavailable" state (§9.1) if every source failed this run, never a
  numeric `$0`.
- Assets and liabilities visually grouped separately.
- **Colorblind-friendly palette:** use the Okabe–Ito palette (blue #0072B2,
  orange #E69F00, sky blue #56B4E9, bluish green #009E73, vermillion #D55E00,
  purple #CC79A7) instead of default red/green. Critically: **never encode
  success/failure by color alone** — pair every status color with an icon or
  text label ("Failed", "Updated 2m ago") so it reads correctly regardless of
  color vision.
- Stretch — trends: requires ≥2 historical snapshots; simple line chart of net
  worth and per-category balances over time once that data exists.
- **No in-TUI refresh in v0.1** — getting new data means quitting and
  rerunning `obol` (§6.2), which fetches, saves, and renders in one shot.
  An in-TUI refresh keybinding is deferred to v0.4 (§15).

**Sources screen:**
- List of configured sources with provider, category, type, and connection
  health (e.g. "Plaid: connected", "Manual: last updated 3 runs ago").
- **Plaid Item usage indicator** (§7.1): "Plaid Items: X/10 used," always
  visible on this screen, not just when adding a source — warns at 8/10,
  blocks new Plaid connections at 10/10.
- Add / Edit / Remove actions per §10.1, including the Plaid Hosted Link
  flow and the confirmed-cleanup remove flow.

## 14. Tech stack (Rust — security-first, decision D6)

| Layer | Choice | Why |
|---|---|---|
| Core (shared) | Plain Rust + `serde`/`serde_json` | No UI dependency in the core library — both interfaces call the same functions; `serde` handles schema + versioned migrations |
| Async runtime | `tokio` | Already your working environment from recent projects |
| Plaid integration | Hand-rolled `reqwest` + `serde` client against Plaid's documented REST endpoints (Balance, Investments, Liabilities, Link) | No official Rust SDK exists (community crates like `rplaid` and OpenAPI-generated clients exist but are unaudited third-party surfaces); a narrow, self-written client covering only the endpoints actually needed keeps the audited surface small — consistent with §4's minimal-dependency principle |
| Browser automation | `fantoccini` (WebDriver protocol, via chromedriver/geckodriver) | No Rust Playwright bindings exist. **Needs a spike**: WebDriver automation has historically struggled more than Playwright's CDP approach against heavy anti-bot/JS bank login flows — validate against a real target before committing (§7) |
| TLS | `rustls` (via `reqwest`'s `rustls-tls` feature) | Memory-safe TLS stack, avoids linking OpenSSL and its associated CVE history |
| Secrets | `secrecy` + `zeroize` | `secrecy` prevents accidental exposure via `Debug`/logging; `zeroize` gives deterministic, compiler-enforced wiping on drop (§4) |
| Keychain | `security-framework` crate | Rust bindings to macOS Security.framework; same `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` ACL as before |
| Retry logic | `tokio-retry` (`RetryIf` + `ExponentialBackoff`) | Same policy as §9 (3 attempts, exponential backoff, jitter); `RetryIf`'s condition closure is what lets auth failures fail fast instead of retrying (D10, §16) — simpler than hand-rolling the same branch |
| **CLI/TUI (phase 1)** | `clap` + `ratatui` | `clap` for scriptable commands (`dashboard snapshot`), `ratatui` for the interactive terminal dashboard/sources screens — mature, well-regarded Rust TUI framework |
| **GUI (phase 2)** | Local HTTP(S) endpoint (`127.0.0.1:<port>`), reachable via browser | Simpler packaging than a native GUI toolkit — no bundled UI framework. **Trade-off to design properly once we get here (decision D7, §16):** unlike a native app, a listening port has real local attack surface (other local processes, DNS-rebinding from a malicious page in another tab) that needs mitigating — session token, strict `Host` header checks, no CDN-loaded assets. Deferred until after v0.1/v0.2 are done, not designed yet |
| Charts | `plotters` | Renders to both the TUI (via `ratatui` widgets) and the GUI canvas; colorblind-safe palette applied manually since it's a lower-level library than Plotly |
| Packaging | `cargo build --release` (native binary) | No interpreter or bundler runtime to ship. **Codesigning/notarization is not needed for v0.1/v0.2**: a binary you build locally and run from Terminal never gets the `com.apple.quarantine` attribute that triggers Gatekeeper — that only applies to files downloaded via a browser, Mail, or AirDrop. This only becomes a real question if the tool is ever distributed as a `.app` for someone else, or downloaded rather than built — and since the "GUI" phase is a browser-reached HTTP endpoint (D7), not a native `.app` bundle, it may never apply to this project at all |
| Dependency management | `Cargo.lock` (committed) | Rust's default toolchain is already lockfile-first, directly reinforcing the supply-chain principle in §4 |
| Logging | `tracing` + `tracing-subscriber` | Backs the audit log requirement (§4) — structured, filterable, and the natural place to enforce "never log a credential or full account number" as a consistent policy rather than an ad-hoc discipline |
| Error handling | `thiserror` in the core library (typed `ProviderError`, `SnapshotError`, etc. — see the `Provider` trait in §10); `anyhow` in the CLI/TUI binary for top-level error reporting | Conventional Rust split: typed, matchable errors in the library; ergonomic error context at the application boundary |
| Concurrency / file locking | `fs2` (or `fslock`) — OS-level advisory lock (`flock`) on a `.lock` file in the storage directory | Serializes concurrent invocations (D13, §9.1) so a scheduled `launchd` run and an interactive session don't race on `sources.yaml`, snapshot writes, or the Plaid Item counter — `std::sync::Mutex` doesn't apply across separate processes |

Given your recent hands-on work with Tokio and async Rust, this stack likely
requires less ramp-up than the Python plan would have, on top of closing the
memory-scrubbing gap (§4). The genuine new ground is the Plaid REST client
and the `fantoccini`-based browser automation — both scoped narrowly and
flagged for early spikes rather than assumed to just work.

## 15. Phased plan

Within every phase below, core-library work follows the test-first
discipline in §5 — write the failing test, then the implementation.
Live-integration work (real Plaid Sandbox calls, the `fantoccini` spike)
is verified separately, not forced into that same red-green loop (§5).

- **v0.1 (core + CLI/TUI):** core library — Plaid Sandbox connector
  (hand-rolled `reqwest`/`serde` client), `manual_entry` provider, snapshot
  engine with retry/PII scrubbing, versioned JSON storage — plus a terminal
  interface (`clap` commands + `ratatui` screens) covering: run a snapshot,
  view net worth and per-account detail, and the Sources screen for
  add/edit/remove (§10.1), including the Plaid Hosted Link connect flow
  (opens your system browser), confirmed Item-removal cleanup on delete,
  and the persistent Item usage indicator (§7.1). Packaged as a standalone
  `cargo build --release` binary so it's runnable without any language
  runtime even at this stage.
  **Also in this phase:** a short, isolated spike validating `fantoccini`
  against one real (non-critical) bank-style login flow, before committing
  to WebDriver automation as the browser-automation approach for v0.2 — if
  it can't reliably clear modern anti-bot/JS flows, this is the point to
  reconsider (§14).
- **v0.2 (production data + browser fallback):** real Plaid Production
  linking (still through the TUI's Hosted Link flow) for
  Chase/Vanguard/Fidelity/E-Trade; `fantoccini`-based fallback connector for
  any source Plaid can't reach well; retry/backoff hardening; full security
  hardening from §4 (file permissions, storage path, dependency pinning)
  verified end-to-end.
- **v0.3 (local HTTP interface):** replaces the native-GUI-binary plan with a
  local HTTP(S) endpoint (`127.0.0.1:<port>`) reachable via browser, wrapping
  the same core library and `sources.yaml` — becomes the primary day-to-day
  interface. The CLI/TUI is not retired; it remains the interface for
  scheduled/headless runs (see §6.1) and continues to share 100% of the core
  with the HTTP interface. **Security design for the HTTP layer (auth token,
  Host-header validation, asset vendoring — see D7, §16) is scoped in detail
  when this phase actually starts, not before** — no point designing it
  against a moving target while v0.1/v0.2 are still in flight.
- **v0.4 (stretch):** asset-type pie chart, spending/savings trend lines,
  student loan + mortgage connectors once servicers are chosen, scheduled
  biweekly runs (`launchd` job calling the CLI, no GUI required), an
  in-TUI manual refresh command (re-fetch without quitting and rerunning
  `obol`, §13), and a configurable storage location (§4, D17).

## 16. Decisions log

Previously open questions, now resolved:

- **D1 — Credit/Debit terminology:** confirmed as Asset/Liability (§11,
  FR4).
- **D2 — Plaid access token persistence:** resolved to Keychain storage
  (§8, §10). Driving factor: the Trial plan's 10-Item cap is a lifetime
  budget, not concurrent — re-running Plaid Link every execution would
  exhaust it within a handful of runs, making the free tier impractical.
  Storing only the Plaid access token (never a bank password) in Keychain,
  scoped and revocable, was the only option compatible with actually using
  the free tier long-term.
- **D3 — Apple Card handling:** kept in v1 as a manual-entry form field each
  run, using the `manual_entry` provider (§10) — same snapshot schema and
  dashboard panel treatment as every automated source, just no credential
  prompt or retry logic involved.
- **D4 — Provider swappability:** the connector layer is split into
  `Source` (config) and `Provider` (implementation) so that moving off
  Plaid — for one institution or entirely — requires a new `Provider` class
  and a config change, never a change to the snapshot engine, storage, PII
  scrubbing, or dashboard (§10).
- **D5 — Interface sequencing:** build CLI/TUI first, second interface
  second (§6.1, §15), both sitting on one core library. Rationale: the
  CLI/TUI validates the whole pipeline cheaply before investing in a second
  interface, and because you're not coming from a Python/Rust-web
  background by default, an interface reachable in a normal browser is the
  intended way you'll use this day-to-day (superseded in shape, not intent,
  by D7 — a local HTTP endpoint rather than a native GUI binary). The
  CLI/TUI isn't a throwaway prototype, though — it stays on as the headless
  interface for scheduled runs (`launchd`), since a biweekly snapshot job
  shouldn't need a browser open.
- **D6 — Language: Rust over Python.** Driven by the "security over
  convenience" principle (FR16, §4): `secrecy`/`zeroize` close the
  memory-scrubbing gap Python can't close, `rustls` avoids OpenSSL's CVE
  history, and native compilation removes the PyInstaller-bundling attack
  surface. Trade-off acknowledged: no official Rust SDK for Plaid (hand-rolled
  client instead) and no Rust bindings for Playwright (`fantoccini`/WebDriver
  instead, needing an early spike). Landing this in Rust also plays to your
  existing Tokio/async experience rather than starting cold.
- **D7 — GUI phase becomes a local HTTP(S) endpoint,** not a native `egui`
  binary — reachable via browser at `127.0.0.1:<port>` instead of a
  double-clickable app window. This trades away the zero-network-surface
  property a native GUI would have had, in exchange for simpler packaging.
  The mitigations that trade-off requires (session token, strict `Host`
  header validation to block DNS rebinding, no CDN-loaded assets) are
  real and necessary, but **deliberately not designed yet** — that work
  starts when v0.3 actually begins, once the CLI/TUI (v0.1) is built and
  the core library is stable, rather than being speculatively designed now.
- **D8 — Plaid Item usage is tracked locally and surfaced in both
  interfaces** (FR21, §7.1). Plaid has no API to query remaining Item
  quota, so a lifetime counter is maintained by the app itself —
  incremented on every successful new Item creation, never decremented on
  removal, since `/item/remove` doesn't free the Trial cap (§7). Both the
  TUI and the eventual HTTP interface show "Plaid Items: X/10 used"
  prominently, with a warning at 8/10 and a hard block on new Plaid
  connections at 10/10. Separately: Production API rate limits (5/min,
  30/hour per Item on `/accounts/balance/get`) were checked and are not a
  practical constraint for biweekly single-user runs — the only scenario
  worth a soft warning is many manual re-runs against the same Item within
  a short debugging session.
- **D9 — Test-driven development for core library logic** (§5): tests are
  written before implementation for everything deterministic and
  network-free — the `Provider` trait contract (via fakes), retry/backoff,
  PII scrubbing, schema serialization/migration, net worth calculation, the
  Item usage counter, and Sources config CRUD. Real third-party integration
  (Plaid Sandbox calls, the `fantoccini` browser-automation spike) is
  verified as a separate, lighter-weight integration tier rather than
  forced into the same test-first loop, since live external systems aren't
  a good fit for red-green TDD.
- **D10 — Retry logic uses the `tokio-retry` crate**, specifically `RetryIf`
  (§9, §14), instead of hand-rolling `tokio::time::sleep` + jitter or
  pulling in `backoff`. `RetryIf`'s condition closure is what gives fail-fast
  auth-failure behavior for free — the alternative was writing that branch
  by hand — while still satisfying the minimal-dependency principle (§4):
  it's a small, single-purpose crate, not a general-purpose framework.
- **D11 — `Asset` and `Liability` share a common `Account` trait** (§11),
  the same interface-based pattern already used for `Provider` (§10) rather
  than a single struct with a `category` enum field branched on throughout
  net worth calc and dashboard rendering. `net_worth_contribution()` on the
  trait centralizes the one piece of sign logic (positive for assets,
  negative for liabilities) that would otherwise be duplicated wherever
  category is checked. Purely a domain-layer abstraction — the on-disk JSON
  schema (§11.2) is unchanged, since storage serializes/deserializes the
  flat DTO shape, not the trait object.
- **D12 — Interactive prompting (webdriver credentials, manual-entry
  balance) is threaded through core via a `CredentialSource` trait**
  (§6.2, §8), rather than giving core a UI-crate dependency or having
  each front end duplicate the per-source orchestration logic. Plaid
  sources bypass this entirely, resolving their access token from
  Keychain internally (D2). This is the mechanism that keeps
  `core::snapshot::run()` — provider instantiation, concurrent per-source
  fetch, retry, PII scrubbing, snapshot assembly — identical between the
  CLI/TUI and the future HTTP interface, per §6.1's "same core, two
  interfaces."
- **D13 — Failure modes beyond per-source retry** (§9.1): total-outage net
  worth display (explicit "unavailable" state, never `$0`), best-effort
  (non-blocking) snapshot persistence, Plaid Keychain failures treated as
  a relink prompt rather than a generic error, a malformed `sources.yaml`
  surfaced as a clear parse error distinct from a single bad source entry
  (which stays per-source, per §9's isolation principle), WebDriver
  infrastructure errors kept distinct from login failures and not
  retried, and concurrent runs serialized via an OS-level advisory file
  lock (`fs2`/`fslock`, §14) rather than left to race on shared state.
- **D14 — Forward compatibility for the snapshot schema** (§11.3):
  alongside the existing backward-compatibility migration chain, the
  loader also tolerates snapshots written by a *newer* binary — unknown
  fields and account types are ignored rather than rejected, so a
  downgrade or a snapshot from a newer version doesn't hard-fail. Same
  "old snapshots must still render" spirit as FR14, extended in the other
  direction.
- **D15 — The account-key salt lives in `sources.yaml`, per source**
  (§11.1): generated once at add-time, not regenerated per run (would
  break run-over-run tracking) and not a single install-wide value (would
  let leaked snapshots from different installs be correlated). Rides on
  the same `0600` protection the file already has — a pragmatic choice for
  now, not necessarily permanent.
- **D16 — Balances stay `f64`** (§11, §12, §14): `rust_decimal`/cents-based
  integers were considered and rejected for v1 — this is a dashboard for a
  glance-at-it figure, not an accounting tool, so sub-cent float drift is
  an accepted tradeoff.
- **D17 — Storage path is fixed, not configurable, in v1** (§4): an
  override mechanism reopens the cloud-sync-folder validation question §4
  exists to close, so it's deferred rather than designed now. Named as a
  v0.4 stretch item (§15) rather than left as an unscoped "someday."
- **D18 — Plaid Link polling is a non-blocking background task, not a
  blocking loop** (§10.1): the 6-hour result-retrieval window is Plaid's
  own guarantee, not an expected completion time — the Hosted Link/QR-code
  flow means a user could genuinely take a while, so the TUI can't freeze
  while waiting. The pending session is cancelable from the Sources
  screen.
- **D19 — No in-TUI manual refresh in v0.1** (§13): getting new data means
  quitting and rerunning `obol`, which is an acceptable workflow for a
  single-user, on-demand v0.1 tool and avoids overlapping a fresh fetch
  with a screen that's mid-render. Named as a v0.4 stretch item (§15)
  rather than left unscoped.
tely-phased feature (v0.5, §16) — not part of v0.1's initial scope.

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
  (stocks/RSUs). See §7 for the connection approach pe# Obol — Financial Health Dashboard: Specification

Status: Ready for implementation
Purpose: Input to design and implementation phases (spec-driven development)

**Starting point for implementation:** begin with the v0.1 scope in §16 —
core library + CLI/TUI. Part 1 (§1–3) is the functional requirements and
should read as the "what" and "why." Part 2 (§4–17) is the "how" — security
design, architecture, tech stack, and phasing. **Development is test-first
for all core-library logic (§5, D9)** — write the failing test before the
implementation. Don't build the HTTP interface (§6.1, §15, D7) until v0.1/v0.2
are complete and its security design has actually been done — see D7's
explicit deferral. Recommendation tracking (§13, FR22–FR27) is a later,
separr institution.
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
  and liabilities visually grouped separately. See §14.
- **FR9** — The UI is colorblind-friendly — status and category are never
  conveyed by color alone. See §14.
- **FR10 (stretch)** — Identify trends in savings/spending, once at least
  two historical snapshots exist. See §14.

### 2.3 Snapshots & history

- **FR11** — Every run creates a snapshot of each connected account.
  Long-term, this runs on a schedule (e.g. biweekly). See §16 (v0.4).
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

### 2.8 Recommendation / financial health tracking

- **FR22** — The dashboard tracks financial-plan recommendations (e.g.
  emergency fund coverage, savings rate, retirement plan probability of
  success, estate-document completion) as discrete metrics with a clear
  status, rather than leaving them static in a one-time PDF. See §13.
- **FR23** — Threshold-based metrics are displayed with a traffic-light
  status (red/yellow/green, or more bands where appropriate), computed from
  either snapshot data or user-supplied inputs. See §13.1, §13.2.
- **FR24** — Checklist-style recommendations (e.g. estate documents,
  insurance in place) are tracked as simple complete/incomplete items, not
  forced into a threshold model that doesn't fit them. See §13.1.
- **FR25** — All thresholds and target values are user-editable, not
  hard-coded to any single financial plan revision, since these change at
  each annual plan review. See §13.2.
- **FR26** — Recommendation status is displayed using the same
  colorblind-friendly, never-color-alone treatment as the rest of the
  dashboard. See §13.2, §14.
- **FR27** — Recommendation tracking rolls out incrementally rather than
  all at once, prioritized by how automatable each recommendation actually
  is. See §13.3, §16.

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
- **Language choice is driven by this requirement (decision D6, §17):**
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
  (use e.g. `~/Library/Application Support/Obol/`) — scrubbed
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
  (§17, D7).
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
  this is exactly what the v0.1 spike (§16) exists to validate; it's a
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
rewrite. See §16 for the phased build order and §15 for the specific
libraries behind each interface.

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

**Access token persistence:** resolved in decision D2 (§17) — Plaid access
tokens are stored in macOS Keychain, scoped and revocable independent of any
real bank credentials. See §17 for the reasoning.

### 7.1 Item usage tracking & API rate limits

**Item usage must be surfaced in both interfaces (FR21, §2.7; decision D8,
§17).** Plaid does not expose an API to query how many of your 10 Trial
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
  D2, §17).
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
- **Per-source isolation:** a failed source produces a `status: "error"` entry
  in the snapshot with a human-readable message; it does not raise or abort
  the run. The dashboard renders every other panel normally and shows a clear
  "data unavailable" state for the failed one (not a blank or crashed panel).

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
    ) -> Result<Vec<AccountBalance>, ProviderError>;
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
    provider_config:
      plaid_institution_id: ins_56

  - id: vanguard_investments
    provider: plaid
    category: asset
    type: brokerage
    institution: vanguard
    provider_config:
      plaid_institution_id: ins_12

  - id: apple_card
    provider: manual_entry
    category: liability
    type: credit_card
    institution: goldman_sachs

  - id: student_loan_navient
    provider: webdriver
    category: liability
    type: student_loan
    institution: navient
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
display name, category, type, institution, provider-specific fields → write
new entry to `sources.yaml` (atomic write — temp file + rename, so a crash
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
   writes the new source entry — no separate webhook server needed for a
   single-user local tool.

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

### 11.1 PII scrubbing rules

Snapshots store **no**: account numbers, account holder name, institution
login identifiers, or raw API response payloads. Snapshots store **only**:
institution name, a locally-generated, **salted** pseudonymous account key
(so the same account can be tracked run-over-run for trend analysis without
storing — or being reversible to — the real account number), category,
type, balance, currency, and timestamp.

Storage hardening (see §4 for full rationale):
- Snapshot and config files written with `0600` permissions, containing
  directory `0700`.
- Default storage path is outside any cloud-synced folder (e.g.
  `~/Library/Application Support/Obol/`), not `~/Documents`,
  `~/Desktop`, iCloud Drive, or Dropbox.

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

## 12. Net worth & breakdown logic

- **Net worth** = sum(asset balances) − sum(liability balances), computed only
  over accounts with `status: "ok"` for that run, with a visible note of which
  sources were excluded due to failure.
- **Stretch — asset breakdown pie chart:** group by `type` (cash, retirement,
  brokerage, 529, RSU/stock), rendered as a pie chart.

## 13. Recommendation tracking (financial health metrics)

Your advisor's plan (and any future revision of it) contains a set of
concrete, checkable recommendations — an emergency fund target, a savings
rate, insurance coverage, estate documents, a retirement plan's probability
of success. Right now these live in a PDF you read once. This section
generalizes them into first-class, continuously-visible metrics — the same
"don't bury the important number" treatment already given to the Plaid Item
usage counter (§7.1), just applied more broadly (FR22, §2.8).

### 13.1 Recommendation types

Not every recommendation fits the same shape, so four distinct kinds are
modeled rather than forcing everything into one:

**A. Threshold metrics computed from snapshot data alone** — no new user
input needed beyond what Obol already fetches:
- Emergency fund coverage: cash + money-market balances (already categorized
  as `checking`/`money_market` types, §11) divided by a target monthly-expense
  figure. Editable default bands: <6 months red, 6–9 months yellow, >9 months
  green (illustrative starting point from the June 2026 plan — see FR25 on
  editability).

**B. Threshold metrics needing a small amount of user-supplied context** —
Obol doesn't and shouldn't try to derive these from balances alone:
- Retirement savings rate: needs annual income and annual retirement
  contributions as inputs, since Plaid's Balance/Investments products don't
  expose "how much did I contribute this year." Editable default bands:
  <10% red, 10–20% yellow, ≥20% green.
- Target monthly spend: this one is explicitly a *moving target*, not a fixed
  threshold — the plan calls for stepping down $500/month toward a floor,
  with a ceiling that changes once Sarah is employed. Modeled as a glidepath
  (start value, step size, floor, and an optional ceiling event) rather than
  a static band.
- 529 education percent-funded: needs the same kind of forward-looking
  calculation your advisor's software already does (current 529 balance,
  projected growth, and total cost) — Obol tracks the percentage as a
  manually-updated number from each plan revision rather than attempting to
  replicate a full education-funding projection itself.

**C. Externally-computed metrics, manually re-entered periodically** — Obol
has no way to calculate these itself; it can only store and display the last
known value plus how stale it is:
- Retirement plan probability of success (from your advisor's Monte Carlo
  tool). Editable default bands: <70% red, 70–85% yellow, >85% green
  (standard industry convention, not specific to your plan).
- Any other advisor-computed projection you want tracked over time.

**D. Checklist items** — complete or incomplete, no threshold at all,
displayed the same way the Plaid Item usage counter shows "X/10":
- Estate documents (Will, Revocable Living Trust, Medical POA, Financial POA,
  Living Will, HIPAA release) — "4/6 complete," etc.
- Own-occupation disability insurance in place
- $1,100,000 term life insurance in place (only relevant once the CA home
  purchase closes — see the activation-condition note below)
- Sarah's WA TRS rollover completed
- Roth election on Kevin's 401(k)
- FSA enrollment
- ESPP participation / immediate-sale discipline

**Activation conditions:** some checklist items only become relevant given
another event (the life insurance need is conditioned on buying the CA
home). Rather than showing an irrelevant item as "incomplete" indefinitely,
each checklist item can optionally declare a simple precondition (e.g. "only
show once `ca_home_purchased` is true") — a small boolean flag you set
yourself, not something Obol infers.

### 13.2 Architecture

A `Recommendation` is the metric-tracking analogue of a `Provider` (§10) —
the same instinct of keeping the "what" (the metric definition) separate
from "how its value is obtained":

```rust
enum ValueSource {
    Snapshot,                                        // type A
    UserConfig,                                       // type B
    ManualExternal { last_updated: DateTime<Utc> },    // type C
    Checklist,                                         // type D
}

struct Recommendation {
    id: String,
    description: String,
    value_source: ValueSource,
    thresholds: Option<ThresholdBands>, // None for checklist items
    precondition: Option<String>,       // e.g. "ca_home_purchased"
}
```

Recommendation definitions and their editable thresholds live in a
`recommendations.yaml`, managed through the UI exactly like `sources.yaml`
(§10.1) — no hand-editing required, and the same atomic-write / `0600`
treatment (§4, §11.1) applies, since threshold values and manually-entered
figures (income, insurance amounts) are meaningfully sensitive even though
they're not credentials.

**Display:** a dedicated screen (§14) shows every recommendation's current
status using the same colorblind-safe, never-color-alone convention as the
rest of the dashboard (icon + label, not color alone) — status bands render
as a short label ("Red — 4.2 months," "6/6 complete," "Stale — last updated
11 months ago" for type C metrics whose freshness matters as much as their
value).

**Editability (FR25):** every threshold band and every manually-entered
value is editable through the same UI, since your plan is reviewed annually
and this June 2026 plan's specific numbers ($70k–$105k, 10–20%, etc.) are
illustrative starting defaults, not permanent constants.

### 13.3 Rollout order

Given the range of automatability across these types, recommendation
tracking rolls out in the order each type actually gets easier to build, not
all at once (per your direction — "all of it, phase it out over time"):

1. **Type A (pure snapshot-derived)** first — emergency fund coverage needs
   no new input at all, since Obol already has categorized cash/money-market
   balances from v0.1 onward.
2. **Type D (checklists)** next — trivial to build (a boolean + optional
   precondition), high value, and directly reuses the "X/N complete"
   pattern already built for Plaid Item usage (§7.1).
3. **Type B (user-config threshold metrics)** — needs a small new config
   surface for income, contributions, and the spend glidepath, but no new
   architectural concept beyond what §13.2 already defines.
4. **Type C (externally-computed, manually re-entered)** last — lowest
   automation value since Obol is just storing a number you type in after
   each advisor meeting, but still worth having in one place rather than
   back in the PDF.

## 14. Dashboard UI

Three screens: the **net worth dashboard** (default view), a **Sources**
screen for managing connections (§10.1), and a **Recommendations** screen
for financial health tracking (§13).

**Dashboard:**
- One panel per source, each independently rendered — a failed source shows a
  clear "unavailable" state with the error message, not a blank space or a
  crashed page.
- Top-level net worth figure, prominent.
- Assets and liabilities visually grouped separately.
- **Colorblind-friendly palette:** use the Okabe–Ito palette (blue #0072B2,
  orange #E69F00, sky blue #56B4E9, bluish green #009E73, vermillion #D55E00,
  purple #CC79A7) instead of default red/green. Critically: **never encode
  success/failure by color alone** — pair every status color with an icon or
  text label ("Failed", "Updated 2m ago") so it reads correctly regardless of
  color vision.
- Stretch — trends: requires ≥2 historical snapshots; simple line chart of net
  worth and per-category balances over time once that data exists.

**Sources screen:**
- List of configured sources with provider, category, type, and connection
  health (e.g. "Plaid: connected", "Manual: last updated 3 runs ago").
- **Plaid Item usage indicator** (§7.1): "Plaid Items: X/10 used," always
  visible on this screen, not just when adding a source — warns at 8/10,
  blocks new Plaid connections at 10/10.
- Add / Edit / Remove actions per §10.1, including the Plaid Hosted Link
  flow and the confirmed-cleanup remove flow.

**Recommendations screen (§13):**
- Every configured recommendation, one row per item, status shown with the
  same colorblind-safe icon + label convention as the main dashboard — never
  color alone.
- Threshold metrics (types A–C, §13.1) show their current value and band
  ("Red — 4.2 months of expenses"); checklist items (type D) show
  completion count ("4/6 estate documents complete").
- Type C metrics additionally show staleness ("last updated 11 months ago")
  since, unlike A/B/D, their value can't be freshened by simply running a
  snapshot.
- Add / Edit / Remove / adjust-thresholds actions, mirroring the Sources
  screen's UI pattern (§10.1) — generated forms per recommendation type
  rather than hand-edited YAML, consistent with FR25's editability
  requirement.

## 15. Tech stack (Rust — security-first, decision D6)

| Layer | Choice | Why |
|---|---|---|
| Core (shared) | Plain Rust + `serde`/`serde_json` | No UI dependency in the core library — both interfaces call the same functions; `serde` handles schema + versioned migrations |
| Async runtime | `tokio` | Already your working environment from recent projects |
| Plaid integration | Hand-rolled `reqwest` + `serde` client against Plaid's documented REST endpoints (Balance, Investments, Liabilities, Link) | No official Rust SDK exists (community crates like `rplaid` and OpenAPI-generated clients exist but are unaudited third-party surfaces); a narrow, self-written client covering only the endpoints actually needed keeps the audited surface small — consistent with §4's minimal-dependency principle |
| Browser automation | `fantoccini` (WebDriver protocol, via chromedriver/geckodriver) | No Rust Playwright bindings exist. **Needs a spike**: WebDriver automation has historically struggled more than Playwright's CDP approach against heavy anti-bot/JS bank login flows — validate against a real target before committing (§7) |
| TLS | `rustls` (via `reqwest`'s `rustls-tls` feature) | Memory-safe TLS stack, avoids linking OpenSSL and its associated CVE history |
| Secrets | `secrecy` + `zeroize` | `secrecy` prevents accidental exposure via `Debug`/logging; `zeroize` gives deterministic, compiler-enforced wiping on drop (§4) |
| Keychain | `security-framework` crate | Rust bindings to macOS Security.framework; same `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` ACL as before |
| Retry logic | Hand-rolled `tokio::time::sleep` + jitter, or the `backoff` crate | Same policy as §9 (3 attempts, exponential backoff, jitter) |
| **CLI/TUI (phase 1)** | `clap` + `ratatui` | `clap` for scriptable commands (`obol snapshot`), `ratatui` for the interactive terminal dashboard/sources screens — mature, well-regarded Rust TUI framework |
| **GUI (phase 2)** | Local HTTP(S) endpoint (`127.0.0.1:<port>`), reachable via browser | Simpler packaging than a native GUI toolkit — no bundled UI framework. **Trade-off to design properly once we get here (decision D7, §17):** unlike a native app, a listening port has real local attack surface (other local processes, DNS-rebinding from a malicious page in another tab) that needs mitigating — session token, strict `Host` header checks, no CDN-loaded assets. Deferred until after v0.1/v0.2 are done, not designed yet |
| Charts | `plotters` | Renders to both the TUI (via `ratatui` widgets) and the GUI canvas; colorblind-safe palette applied manually since it's a lower-level library than Plotly |
| Packaging | `cargo build --release` (native binary) | No interpreter or bundler runtime to ship. **Codesigning/notarization is not needed for v0.1/v0.2**: a binary you build locally and run from Terminal never gets the `com.apple.quarantine` attribute that triggers Gatekeeper — that only applies to files downloaded via a browser, Mail, or AirDrop. This only becomes a real question if the tool is ever distributed as a `.app` for someone else, or downloaded rather than built — and since the "GUI" phase is a browser-reached HTTP endpoint (D7), not a native `.app` bundle, it may never apply to this project at all |
| Dependency management | `Cargo.lock` (committed) | Rust's default toolchain is already lockfile-first, directly reinforcing the supply-chain principle in §4 |
| Logging | `tracing` + `tracing-subscriber` | Backs the audit log requirement (§4) — structured, filterable, and the natural place to enforce "never log a credential or full account number" as a consistent policy rather than an ad-hoc discipline |
| Error handling | `thiserror` in the core library (typed `ProviderError`, `SnapshotError`, etc. — see the `Provider` trait in §10); `anyhow` in the CLI/TUI binary for top-level error reporting | Conventional Rust split: typed, matchable errors in the library; ergonomic error context at the application boundary |

Given your recent hands-on work with Tokio and async Rust, this stack likely
requires less ramp-up than the Python plan would have, on top of closing the
memory-scrubbing gap (§4). The genuine new ground is the Plaid REST client
and the `fantoccini`-based browser automation — both scoped narrowly and
flagged for early spikes rather than assumed to just work.

## 16. Phased plan

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
  reconsider (§15).
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
  Host-header validation, asset vendoring — see D7, §17) is scoped in detail
  when this phase actually starts, not before** — no point designing it
  against a moving target while v0.1/v0.2 are still in flight.
- **v0.4 (stretch):** asset-type pie chart, spending/savings trend lines,
  student loan + mortgage connectors once servicers are chosen, scheduled
  biweekly runs (`launchd` job calling the CLI, no GUI required).
- **v0.5 (recommendation tracking, §13):** rolled out in the order given in
  §13.3, each step a self-contained, shippable increment rather than one
  large feature drop:
  - **v0.5a** — Type A (snapshot-derived) metrics: emergency fund coverage,
    using cash/money-market balances already available since v0.1.
  - **v0.5b** — Type D (checklists): estate documents, insurance in place,
    and the other one-time action items, reusing the "X/N complete" pattern
    from the Item usage counter (§7.1).
  - **v0.5c** — Type B (user-config threshold metrics): savings rate and
    the spending glidepath, adding the small config surface for income and
    contributions that these require.
  - **v0.5d** — Type C (externally-computed, manually re-entered): retirement
    plan probability of success and similar advisor-computed figures, plus
    the staleness indicator for values that aren't refreshed by a snapshot
    run.

## 17. Decisions log

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
  second (§6.1, §16), both sitting on one core library. Rationale: the
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
- **D10 — Recommendation tracking added as a new feature area** (FR22–FR27,
  §13), generalizing the specific recommendations from a June 2026 advisor
  plan (emergency fund, savings rate, retirement probability of success,
  estate documents, insurance) into a `Recommendation` abstraction with four
  distinct value sources (snapshot-derived, user-config, manually-tracked
  external, checklist) rather than one threshold model forced onto
  everything. Two explicit decisions within this: **(a)** all thresholds
  and manually-entered figures are user-editable, never hard-coded, since
  financial plans are reviewed annually and this June 2026 plan's specific
  numbers are illustrative defaults, not constants; **(b)** rollout is
  phased by automatability (v0.5a–d, §16) rather than delivered as one
  large feature, starting with what's already computable from existing
  snapshot data and ending with the hardest-to-automate, manually-tracked
  external metrics.


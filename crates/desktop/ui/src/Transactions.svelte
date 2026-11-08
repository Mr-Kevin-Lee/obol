<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import CategoryBars from './CategoryBars.svelte'
  import { insideApp, notInAppMessage } from './backend'
  import { localToday } from './calendar'
  import { formatDate, formatMoney, formatMonth, formatOrdinal, formatSignedMoney } from './format'
  import type { EarlierResult, ImportResult, TransactionRow, TransactionsView } from './types'

  let { month }: { month: string } = $props()

  let view = $state<TransactionsView | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  // The merchant whose category is being saved. One edit at a time, so a
  // slower earlier response can't overwrite a later one's view.
  let pending = $state<string | null>(null)
  // The one category shown, or every one. The dropdown, the By category
  // bars and the Uncategorized-only button all set this, so they can never
  // disagree. Kept across months, so you can follow a category over time.
  let categoryFilter = $state<string | null>(null)

  const UNCATEGORIZED = 'Uncategorized'
  // Money moved or earned, not spent: the same two Spent above leaves out.
  const NOT_SPENDING = new Set(['Transfer', 'Income'])

  // Spending that isn't a bill confirmed in the Spending plan. Combines
  // with the category filter.
  let discretionaryOnly = $state(false)

  function clearFilters() {
    categoryFilter = null
    discretionaryOnly = false
  }

  // Choosing the category already shown clears the filter.
  function toggleCategory(category: string) {
    categoryFilter = categoryFilter === category ? null : category
  }

  // Responses for a month you've already navigated away from are dropped.
  let latest = 0

  $effect(() => {
    load(month)
  })

  async function load(m: string) {
    const request = ++latest
    view = null
    error = null
    loading = true
    if (!insideApp()) {
      error = notInAppMessage
      loading = false
      return
    }
    try {
      const result = await invoke<TransactionsView>('get_transactions', { month: m })
      if (request === latest) view = result
    } catch (e) {
      if (request === latest) error = `Couldn't load transactions: ${e}`
    } finally {
      if (request === latest) loading = false
    }
  }

  async function setCategory(row: TransactionRow, select: HTMLSelectElement) {
    const request = latest
    pending = row.merchant_key
    error = null
    try {
      const result = await invoke<TransactionsView>('set_merchant_category', {
        month,
        merchantKey: row.merchant_key,
        category: select.value,
      })
      if (request === latest) view = result
    } catch (e) {
      // The old view stays; put the picker back so it matches it.
      select.value = row.category
      if (request === latest) error = `Couldn't change the category for ${row.merchant_name}: ${e}`
    } finally {
      pending = null
    }
  }

  // Marking confirms the merchant as a bill in the Spending plan, starting
  // from this payment's amount and day; unmarking removes it from the plan.
  // Either way every payment to the merchant follows, as in the plan.
  async function toggleBill(row: TransactionRow) {
    const request = latest
    pending = row.merchant_key
    error = null
    const args = { month, today: localToday(), merchantKey: row.merchant_key }
    try {
      if (row.bill) {
        await invoke('remove_series', args)
      } else {
        await invoke('confirm_series', {
          ...args,
          kind: 'bill',
          expectedAmount: -row.amount,
          dueDay: Number(row.posted.slice(8, 10)),
        })
      }
      const result = await invoke<TransactionsView>('get_transactions', { month })
      if (request === latest) view = result
    } catch (e) {
      if (request === latest) {
        error = `Couldn't ${row.bill ? 'unmark' : 'mark'} ${row.merchant_name} as a bill: ${e}`
      }
    } finally {
      pending = null
    }
  }

  // Just this one transaction, unlike a category or a bill.
  async function toggleExcluded(row: TransactionRow) {
    const request = latest
    pending = row.merchant_key
    error = null
    try {
      const result = await invoke<TransactionsView>('set_transaction_excluded', {
        month,
        sourceId: row.source_id,
        id: row.id,
        excluded: !row.excluded,
      })
      if (request === latest) view = result
    } catch (e) {
      if (request === latest) {
        error = `Couldn't ${row.excluded ? 'include' : 'exclude'} the ${row.merchant_name} transaction: ${e}`
      }
    } finally {
      pending = null
    }
  }

  let fileInput = $state<HTMLInputElement | null>(null)
  let importing = $state(false)
  let imported = $state<string | null>(null)

  // Categories only, and additive: a merchant you've already categorized
  // here keeps its category. The picker hands over the file itself, so the
  // app needs no path and no access to the folder it's in.
  async function importSimplifi() {
    const file = fileInput?.files?.[0]
    if (!file) return
    importing = true
    error = null
    imported = null
    try {
      const result = await invoke<ImportResult>('import_simplifi', { contents: await file.text() })
      imported = `Learned ${result.rules_added} merchant categories from ${file.name}.`
      await load(month)
    } catch (e) {
      error = `Couldn't read ${file.name}: ${e}`
    } finally {
      importing = false
      // So choosing the same file again still fires a change.
      if (fileInput) fileInput.value = ''
    }
  }

  let loadingEarlier = $state(false)
  let earlierNote = $state<string | null>(null)

  // One more SimpleFIN request each time: the 90 days before what's fetched.
  async function loadEarlier() {
    loadingEarlier = true
    error = null
    earlierNote = null
    try {
      const result = await invoke<EarlierResult>('load_earlier_transactions')
      earlierNote =
        result.added > 0
          ? `Loaded ${result.added} earlier transactions.`
          : `SimpleFIN had no transactions in the 3 months before ${formatDate(view?.simplefin_history_from ?? result.history_from)}.`
      await load(month)
    } catch (e) {
      error = `Couldn't load earlier transactions: ${e}`
    } finally {
      loadingEarlier = false
    }
  }

  // A client-side filter over the rows the view returned.
  const rows = $derived(
    (view?.transactions ?? []).filter(
      (t) =>
        (categoryFilter === null || t.category === categoryFilter) &&
        (!discretionaryOnly ||
          (!t.bill && !t.has_target && !t.excluded && !NOT_SPENDING.has(t.category))),
    ),
  )
  // The view arrives newest first. Amount sorts on the signed value, so
  // ascending puts the most money out first and income last. Sorting is
  // stable, so equal amounts stay newest first.
  type SortKey = 'date' | 'amount'
  let sort = $state<{ key: SortKey; ascending: boolean }>({ key: 'date', ascending: false })

  // A new column starts at its most useful end; the same column reverses.
  function sortBy(key: SortKey) {
    sort = sort.key === key ? { key, ascending: !sort.ascending } : { key, ascending: key === 'amount' }
  }

  const sorted = $derived(
    [...rows].sort((a, b) => {
      const order = sort.key === 'date' ? a.posted.localeCompare(b.posted) : a.amount - b.amount
      return sort.ascending ? order : -order
    }),
  )
  const ariaSort = (key: SortKey) =>
    sort.key === key ? (sort.ascending ? 'ascending' : 'descending') : undefined

  const filtering = $derived(categoryFilter !== null || discretionaryOnly)
  const bars = $derived(
    (discretionaryOnly ? view?.discretionary_categories : view?.categories) ?? [],
  )
  // Net spending of the rows shown, counted the way Spent is: refunds
  // reduce it; transfers, income and excluded rows don't count.
  const shownSpent = $derived(
    rows
      .filter((t) => !t.excluded && !NOT_SPENDING.has(t.category))
      .reduce((sum, t) => sum - t.amount, 0),
  )

  // The dropdown lists this month's categories, most transactions first,
  // plus the current filter even when this month has none of it.
  const filterOptions = $derived.by(() => {
    const counts = new Map<string, number>()
    for (const t of view?.transactions ?? []) counts.set(t.category, (counts.get(t.category) ?? 0) + 1)
    if (categoryFilter !== null && !counts.has(categoryFilter)) counts.set(categoryFilter, 0)
    return [...counts].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
  })
</script>

{#if error}
  <div class="banner error sticky" role="alert">{error}</div>
{/if}

{#if loading}
  <p class="placeholder">Loading…</p>
{:else if view && view.transactions.length === 0}
  <div class="empty">
    <h2>No transactions in {formatMonth(view.month)}</h2>
    <p>Nothing has posted to your accounts for this month.</p>
  </div>
{:else if view}
  <section class="spent" aria-labelledby="spent-heading">
    <h2 id="spent-heading" class="eyebrow">{discretionaryOnly ? 'Discretionary spending' : 'Spent'}</h2>
    <p class="figure">{formatMoney(discretionaryOnly ? view.discretionary_spent_total : view.spent_total)}</p>
  </section>

  {#if bars.length > 0}
    <section class="card" aria-labelledby="categories-heading">
      <h2 id="categories-heading">By category</h2>
      <CategoryBars
        {bars}
        label="Spending by category; choose one to filter the transactions"
        selected={categoryFilter}
        onselect={toggleCategory}
      />
    </section>
  {/if}

  <section class="card" aria-labelledby="list-heading">
    <div class="list-header">
      <h2 id="list-heading">Transactions</h2>
      <div class="filters">
        <label>
          Category
          <select
            value={categoryFilter ?? ''}
            onchange={(e) => (categoryFilter = e.currentTarget.value || null)}
          >
            <option value="">All categories</option>
            {#each filterOptions as [category, count] (category)}
              <option value={category}>{category} ({count})</option>
            {/each}
          </select>
        </label>
        <button
          type="button"
          class="secondary"
          aria-pressed={categoryFilter === UNCATEGORIZED}
          onclick={() => toggleCategory(UNCATEGORIZED)}
        >
          <span aria-hidden="true">{categoryFilter === UNCATEGORIZED ? '☑' : '☐'}</span>
          Uncategorized only ({view.uncategorized_count})
        </button>
        <button
          type="button"
          class="secondary"
          aria-pressed={discretionaryOnly}
          title="Hides bills confirmed in the Spending plan, transfers and income"
          onclick={() => (discretionaryOnly = !discretionaryOnly)}
        >
          <span aria-hidden="true">{discretionaryOnly ? '☑' : '☐'}</span>
          Discretionary only
        </button>
      </div>
    </div>

    {#if filtering && rows.length > 0}
      <p class="shown" role="status">
        {rows.length} {rows.length === 1 ? 'transaction' : 'transactions'} · {formatMoney(shownSpent)} spent
      </p>
    {/if}

    {#if rows.length === 0}
      <p class="none">
        {#if categoryFilter === UNCATEGORIZED && !discretionaryOnly}
          <span aria-hidden="true">✓</span> Nothing to review — every transaction this month has a category.
        {:else}
          No transactions in {formatMonth(view.month)} match these filters.
          <button type="button" class="link" onclick={clearFilters}>Show all</button>
        {/if}
      </p>
    {:else}
      <table>
        <thead>
          <tr>
            <th scope="col" aria-sort={ariaSort('date')}>
              <button type="button" class="sort" onclick={() => sortBy('date')}>
                Date{#if sort.key === 'date'}<span aria-hidden="true">{sort.ascending ? ' ↑' : ' ↓'}</span>{/if}
              </button>
            </th>
            <th scope="col">Merchant</th>
            <th scope="col">Account</th>
            <th scope="col">Category</th>
            <th scope="col"><span class="visually-hidden">Bill</span></th>
            <th scope="col" class="amount" aria-sort={ariaSort('amount')}>
              <button type="button" class="sort" onclick={() => sortBy('amount')}>
                Amount{#if sort.key === 'amount'}<span aria-hidden="true">{sort.ascending ? ' ↑' : ' ↓'}</span>{/if}
              </button>
            </th>
          </tr>
        </thead>
        <tbody>
          {#each sorted as row (`${row.source_id}:${row.id}`)}
            <tr class:excluded={row.excluded}>
              <td class="date">{formatDate(row.posted)}</td>
              <td>{row.merchant_name}</td>
              <td class="muted">{row.account_label}</td>
              <td>
                <select
                  aria-label="Category for {row.merchant_name}"
                  value={row.category}
                  disabled={pending !== null}
                  onchange={(e) => setCategory(row, e.currentTarget)}
                >
                  {#each view.available_categories as category (category)}
                    <option value={category}>{category}</option>
                  {/each}
                </select>
                {#if pending === row.merchant_key}
                  <span class="saving" role="status">Saving…</span>
                {/if}
              </td>
              <td class="actions">
                {#if row.amount < 0 && !row.excluded && !NOT_SPENDING.has(row.category)}
                  <button
                    type="button"
                    class="toggle"
                    aria-pressed={row.bill}
                    aria-label="Payments to {row.merchant_name} are a bill"
                    title={row.bill
                      ? `Payments to ${row.merchant_name} count as a bill; choose to remove it from the Spending plan`
                      : `Add ${row.merchant_name} to the Spending plan as a bill of ${formatMoney(-row.amount)} due on the ${formatOrdinal(Number(row.posted.slice(8, 10)))}`}
                    disabled={pending !== null}
                    onclick={() => toggleBill(row)}
                  >
                    {row.bill ? '✓ Bill' : '+ Bill'}
                  </button>
                {/if}
                {#if row.category !== 'Transfer'}
                  <button
                    type="button"
                    class="toggle"
                    aria-pressed={row.excluded}
                    aria-label="Exclude this {row.merchant_name} transaction"
                    title={row.excluded
                      ? 'Left out of spending and the Spending plan; choose to count it again'
                      : 'Leave just this transaction out of spending and the Spending plan'}
                    disabled={pending !== null}
                    onclick={() => toggleExcluded(row)}
                  >
                    {row.excluded ? 'Excluded' : 'Exclude'}
                  </button>
                {/if}
              </td>
              <td class="amount" class:inflow={row.amount > 0}>
                {formatSignedMoney(row.amount)}
                {#if row.amount > 0}<span class="cue">in</span>{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
{/if}

{#if !loading && view?.simplefin_history_from}
  <section class="history" aria-label="SimpleFIN history">
    <p>
      Transactions from SimpleFIN go back to {formatDate(view.simplefin_history_from)}.
    </p>
    <button type="button" class="secondary" onclick={loadEarlier} disabled={loadingEarlier}>
      {loadingEarlier ? 'Loading…' : 'Load 3 earlier months'}
    </button>
  </section>
{/if}
{#if earlierNote}
  <p class="earlier-note" role="status">{earlierNote}</p>
{/if}

{#if !loading && insideApp()}
  <section class="card" aria-labelledby="import-heading">
    <h2 id="import-heading">Learn categories from Simplifi</h2>
    <div class="import">
      <button type="button" disabled={importing} onclick={() => fileInput?.click()}>
        {importing ? 'Learning…' : 'Choose Simplifi export…'}
      </button>
      <input
        bind:this={fileInput}
        type="file"
        accept=".csv,text/csv"
        hidden
        onchange={importSimplifi}
      />
    </div>
    <p class="hint">
      Reads only how you categorized each merchant in Simplifi; your transactions come from
      SimpleFIN. Categories you've already set here are kept.
    </p>
    {#if imported}
      <p class="imported" role="status">{imported}</p>
    {/if}
  </section>
{/if}

<style>
  .history {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin: 1rem 0 0;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .history p {
    margin: 0;
  }

  .history button {
    font-size: 0.8125rem;
    padding: 0.25rem 0.75rem;
  }

  .earlier-note {
    margin: 0.5rem 0 0;
    font-size: 0.8125rem;
    color: var(--positive);
  }

  .import button {
    font-size: 0.8125rem;
    padding: 0.25rem 0.75rem;
  }

  .hint,
  .imported {
    margin: 0.5rem 0 0;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .imported {
    color: var(--positive);
  }

  .spent {
    padding: 1.25rem 0 0;
  }

  .figure {
    margin: 0.25rem 0 0;
    font-size: 3rem;
    font-weight: 600;
    letter-spacing: -0.025em;
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
  }

  .list-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.75rem;
  }

  .list-header h2 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  .list-header button {
    font-size: 0.8125rem;
    padding: 0.25rem 0.75rem;
  }

  .list-header button[aria-pressed='true'] {
    border-color: var(--blue);
    font-weight: 600;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }

  .filters label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .shown {
    margin: 0 0 0.5rem;
    font-size: 0.8125rem;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }

  /* A text-style action inside a sentence. */
  button.link {
    border: none;
    background: none;
    padding: 0;
    color: var(--blue);
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875rem;
  }

  th {
    text-align: left;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--text-muted);
    padding: 0 0.5rem 0.5rem;
    border-bottom: 1px solid var(--border);
  }

  /* A header that sorts: looks like the other headers. */
  button.sort {
    border: none;
    background: none;
    padding: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
  }

  button.sort:hover {
    color: var(--text);
  }

  th[aria-sort] {
    color: var(--text);
  }

  td {
    padding: 0.4rem 0.5rem;
    border-bottom: 1px solid var(--border);
  }

  tr:last-child td {
    border-bottom: none;
  }

  .date {
    white-space: nowrap;
    color: var(--text-muted);
  }

  .muted {
    color: var(--text-muted);
  }

  .amount {
    text-align: right;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .inflow {
    color: var(--positive);
  }

  .cue {
    font-size: 0.75rem;
    font-weight: 600;
  }

  .saving {
    margin-left: 0.375rem;
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  /* Quiet until it's on, so a column of them doesn't shout. */
  button.toggle {
    font-size: 0.75rem;
    padding: 0.125rem 0.5rem;
    white-space: nowrap;
    color: var(--text-muted);
    border-color: transparent;
    background: none;
  }

  button.toggle:hover:not(:disabled) {
    border-color: var(--border);
  }

  button.toggle[aria-pressed='true'] {
    color: var(--blue);
    border-color: var(--blue);
    font-weight: 600;
  }

  .actions {
    white-space: nowrap;
  }

  /* Still listed so it can be counted again, but visibly out of the sums. */
  tr.excluded td:not(.actions) {
    opacity: 0.5;
  }

  tr.excluded .amount {
    text-decoration: line-through;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .none {
    color: var(--text-muted);
    margin: 0.5rem 0;
  }
</style>

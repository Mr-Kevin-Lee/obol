<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import CategoryBars from './CategoryBars.svelte'
  import { insideApp, notInAppMessage } from './backend'
  import { formatDate, formatMoney, formatMonth, formatSignedMoney } from './format'
  import type { ImportResult, TransactionRow, TransactionsView } from './types'

  let { month }: { month: string } = $props()

  let view = $state<TransactionsView | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  // The merchant whose category is being saved. One edit at a time, so a
  // slower earlier response can't overwrite a later one's view.
  let pending = $state<string | null>(null)
  let onlyUncategorized = $state(false)

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

  let importPath = $state('')
  let importing = $state(false)
  let imported = $state<string | null>(null)

  // Additive, so importing a newer export later only adds what's new.
  async function importSimplifi(event: SubmitEvent) {
    event.preventDefault()
    importing = true
    error = null
    imported = null
    try {
      const result = await invoke<ImportResult>('import_simplifi', { path: importPath })
      imported = `Imported ${result.added} new transactions and ${result.rules_added} new merchant rules.`
      importPath = ''
      await load(month)
    } catch (e) {
      error = `Couldn't import the Simplifi export: ${e}`
    } finally {
      importing = false
    }
  }

  // The review queue: a client-side filter over the rows the view returned.
  const rows = $derived(
    view === null
      ? []
      : onlyUncategorized
        ? view.transactions.filter((t) => t.category === 'Uncategorized')
        : view.transactions,
  )
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
    <h2 id="spent-heading" class="eyebrow">Spent</h2>
    <p class="figure">{formatMoney(view.spent_total)}</p>
  </section>

  {#if view.categories.length > 0}
    <section class="card" aria-labelledby="categories-heading">
      <h2 id="categories-heading">By category</h2>
      <CategoryBars bars={view.categories} label="Spending by category" />
    </section>
  {/if}

  <section class="card" aria-labelledby="list-heading">
    <div class="list-header">
      <h2 id="list-heading">Transactions</h2>
      <button
        type="button"
        class="secondary"
        aria-pressed={onlyUncategorized}
        onclick={() => (onlyUncategorized = !onlyUncategorized)}
      >
        <span aria-hidden="true">{onlyUncategorized ? '☑' : '☐'}</span>
        Uncategorized only ({view.uncategorized_count})
      </button>
    </div>

    {#if rows.length === 0}
      <p class="none">
        <span aria-hidden="true">✓</span> Nothing to review — every transaction this month has a category.
      </p>
    {:else}
      <table>
        <thead>
          <tr>
            <th scope="col">Date</th>
            <th scope="col">Merchant</th>
            <th scope="col">Account</th>
            <th scope="col">Category</th>
            <th scope="col" class="amount">Amount</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as row (`${row.source_id}:${row.id}`)}
            <tr>
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

{#if !loading && insideApp()}
  <section class="card" aria-labelledby="import-heading">
    <h2 id="import-heading">Import from Simplifi</h2>
    <form class="import" onsubmit={importSimplifi}>
      <label>
        Export file
        <input
          type="text"
          placeholder="Full path to the exported .csv file"
          spellcheck="false"
          autocomplete="off"
          bind:value={importPath}
        />
      </label>
      <button type="submit" disabled={importing || importPath.trim() === ''}>
        {importing ? 'Importing…' : 'Import'}
      </button>
    </form>
    <p class="hint">Adds only transactions not already here, and keeps every category you've set.</p>
    {#if imported}
      <p class="imported" role="status">{imported}</p>
    {/if}
  </section>
{/if}

<style>
  .import {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }

  .import label {
    display: flex;
    flex: 1;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .import input {
    flex: 1;
    min-width: 16rem;
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
    font-size: 2.25rem;
    font-weight: 600;
    letter-spacing: -0.02em;
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
    font-size: 1rem;
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

  .none {
    color: var(--text-muted);
    margin: 0.5rem 0;
  }
</style>

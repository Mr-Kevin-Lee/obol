<script lang="ts">
  import { invoke } from '@tauri-apps/api/core'
  import CategoryBars from './CategoryBars.svelte'
  import { insideApp, notInAppMessage } from './backend'
  import { localToday } from './calendar'
  import {
    barWidth,
    formatMoney,
    formatOrdinal,
    formatPercentage,
    formatSignedMoney,
  } from './format'
  import type { PaymentStatus, SeriesKind, SpendingPlanView, SuggestionRow } from './types'

  let { month }: { month: string } = $props()

  let view = $state<SpendingPlanView | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  // One mutation at a time, so a slower earlier response can't overwrite
  // a later one's view.
  let busy = $state(false)

  let editing = $state<string | null>(null)
  let draft = $state<number | null>(null)
  let newCategory = $state('')
  let newTarget = $state<number | null>(null)

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
    editing = null
    if (!insideApp()) {
      error = notInAppMessage
      loading = false
      return
    }
    try {
      const result = await invoke<SpendingPlanView>('get_spending_plan', {
        month: m,
        today: localToday(),
      })
      if (request === latest) view = result
    } catch (e) {
      if (request === latest) error = `Couldn't load the spending plan: ${e}`
    } finally {
      if (request === latest) loading = false
    }
  }

  /** Runs a plan mutation; on failure the old view stays and the banner says why. */
  async function mutate(cmd: string, args: Record<string, unknown>, failure: string) {
    const request = latest
    busy = true
    error = null
    try {
      const result = await invoke<SpendingPlanView>(cmd, { month, today: localToday(), ...args })
      if (request === latest) view = result
      return true
    } catch (e) {
      if (request === latest) error = `${failure}: ${e}`
      return false
    } finally {
      busy = false
    }
  }

  const validAmount = (n: number | null): n is number =>
    typeof n === 'number' && Number.isFinite(n) && n >= 0

  async function saveTarget(category: string, target: number | null) {
    const action = target === null ? 'remove' : 'save'
    if (await mutate('set_category_target', { category, target }, `Couldn't ${action} the ${category} target`)) {
      editing = null
    }
  }

  async function addTarget() {
    if (!validAmount(newTarget)) return
    if (await mutate('set_category_target', { category: newCategory, target: newTarget }, `Couldn't add a ${newCategory} target`)) {
      newCategory = ''
      newTarget = null
    }
  }

  function startEdit(category: string, target: number) {
    editing = category
    draft = target
  }

  const confirm = (s: SuggestionRow) =>
    mutate(
      'confirm_series',
      { merchantKey: s.merchant_key, kind: s.kind, expectedAmount: s.expected_amount, dueDay: s.due_day },
      `Couldn't confirm ${s.name}`,
    )

  const dismiss = (s: SuggestionRow) =>
    mutate('dismiss_series', { merchantKey: s.merchant_key }, `Couldn't dismiss ${s.name}`)

  // Status is always text plus an icon, with colour only reinforcing it (FR9, FR27).
  const statusLabel: Record<PaymentStatus, { icon: string; text: string }> = {
    paid: { icon: '✓', text: 'Paid' },
    upcoming: { icon: '◷', text: 'Upcoming' },
    past_due: { icon: '!', text: 'Past due' },
  }

  const kindLabel: Record<SeriesKind, string> = { bill: 'Bill', income: 'Income' }

  const seriesGroups: { kind: SeriesKind; title: string }[] = [
    { kind: 'income', title: 'Income' },
    { kind: 'bill', title: 'Bills' },
  ]

  // The picker offers only categories that don't have a target yet.
  const untargeted = $derived(
    view === null
      ? []
      : view.available_categories.filter((c) => !view!.targets.some((t) => t.category === c)),
  )
</script>

{#if error}
  <div class="banner error sticky" role="alert">{error}</div>
{/if}

{#if loading}
  <p class="placeholder">Loading…</p>
{:else if view}
  <section class="card summary" aria-labelledby="left-heading">
    <dl>
      <div><dt>Income</dt><dd>{formatMoney(view.income)}</dd></div>
      <div><dt>Bills</dt><dd>{formatMoney(view.bills)}</dd></div>
      <div><dt>Planned</dt><dd>{formatMoney(view.planned)}</dd></div>
      <div><dt>Other</dt><dd>{formatMoney(view.other)}</dd></div>
    </dl>

    <div class="left" class:overspent={view.left < 0}>
      <h2 id="left-heading" class="eyebrow">Left this month</h2>
      <p class="figure">
        {view.left < 0 ? formatSignedMoney(view.left) : formatMoney(view.left)}
      </p>
      {#if view.left < 0}
        <p class="cue"><span aria-hidden="true">▼</span> Overspent</p>
      {/if}
      {#if view.committed_percentage !== null}
        <span class="track" aria-hidden="true">
          <span class="fill" style:width={barWidth(view.committed_percentage)}></span>
        </span>
        <p class="detail">{formatPercentage(view.committed_percentage)} of income committed</p>
      {/if}
      {#if view.per_day !== null}
        <p class="detail">
          {view.per_day < 0 ? formatSignedMoney(view.per_day) : formatMoney(view.per_day)} per day
          for the rest of the month
        </p>
      {/if}
    </div>
  </section>

  {#each seriesGroups as group (group.kind)}
    {@const rows = view.series.filter((s) => s.kind === group.kind)}
    {#if rows.length > 0}
      <section class="card" aria-labelledby="series-{group.kind}">
        <h2 id="series-{group.kind}">{group.title}</h2>
        <ul class="rows">
          {#each rows as s (s.merchant_key)}
            <li>
              <div class="name">
                <span>{s.name}</span>
                <span class="muted">
                  {group.kind === 'income' ? 'Expected' : 'Due'} the {formatOrdinal(s.due_day)}
                </span>
              </div>
              <span class="amount">{formatMoney(s.amount)}</span>
              <span class="status {s.status}">
                <span aria-hidden="true">{statusLabel[s.status].icon}</span>
                {statusLabel[s.status].text}
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/each}

  <section class="card" aria-labelledby="targets-heading">
    <h2 id="targets-heading">Targets</h2>
    {#if view.targets.length === 0}
      <p class="none">No targets yet. Add one to plan a category's spending each month.</p>
    {:else}
      <ul class="targets">
        {#each view.targets as t (t.category)}
          <li class:overspent={t.overspent}>
            <div class="target-row">
              <span class="name">{t.category}</span>
              <span class="spent">
                spent {formatMoney(t.spent)} of {formatMoney(t.target)}
                {#if t.overspent}
                  <span class="cue"><span aria-hidden="true">▲</span> Overspent</span>
                {/if}
              </span>
            </div>
            <span class="track" aria-hidden="true">
              <span class="fill" style:width={barWidth(t.percentage)}></span>
            </span>

            {#if editing === t.category}
              <form
                class="edit"
                onsubmit={(e) => {
                  e.preventDefault()
                  if (validAmount(draft)) saveTarget(t.category, draft)
                }}
              >
                <label>
                  Monthly target for {t.category}
                  <input type="number" min="0" step="0.01" bind:value={draft} />
                </label>
                <button type="submit" disabled={busy || !validAmount(draft)}>Save</button>
                <button type="button" class="secondary" onclick={() => (editing = null)}>
                  Cancel
                </button>
              </form>
            {:else}
              <div class="actions">
                <button
                  type="button"
                  class="secondary"
                  aria-label="Edit the {t.category} target"
                  disabled={busy}
                  onclick={() => startEdit(t.category, t.target)}
                >
                  Edit
                </button>
                <button
                  type="button"
                  class="secondary"
                  aria-label="Remove the {t.category} target"
                  disabled={busy}
                  onclick={() => saveTarget(t.category, null)}
                >
                  Remove
                </button>
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if untargeted.length > 0}
      <form
        class="add"
        onsubmit={(e) => {
          e.preventDefault()
          addTarget()
        }}
      >
        <h3>Add a target</h3>
        <label>
          Category
          <select bind:value={newCategory}>
            <option value="" disabled>Choose…</option>
            {#each untargeted as category (category)}
              <option value={category}>{category}</option>
            {/each}
          </select>
        </label>
        <label>
          Monthly target
          <input type="number" min="0" step="0.01" bind:value={newTarget} />
        </label>
        <button type="submit" disabled={busy || newCategory === '' || !validAmount(newTarget)}>
          Add target
        </button>
      </form>
    {/if}
  </section>

  <section class="card" aria-labelledby="other-heading">
    <h2 id="other-heading">Other spending</h2>
    {#if view.other_categories.length === 0}
      <p class="none">No spending outside your targets this month.</p>
    {:else}
      <CategoryBars bars={view.other_categories} label="Spending without a target" />
    {/if}
  </section>

  <section class="card" aria-labelledby="suggestions-heading">
    <h2 id="suggestions-heading">Suggested bills and income</h2>
    {#if view.suggestions.length === 0}
      <p class="none">No new suggestions.</p>
    {:else}
      <ul class="rows">
        {#each view.suggestions as s (s.merchant_key)}
          <li>
            <div class="name">
              <span>{s.name}</span>
              <span class="muted">
                {kindLabel[s.kind]} · about {formatMoney(s.expected_amount)} · due the
                {formatOrdinal(s.due_day)}
              </span>
            </div>
            <div class="actions">
              <button
                type="button"
                aria-label="Confirm {s.name} as {s.kind === 'bill' ? 'a bill' : 'income'}"
                disabled={busy}
                onclick={() => confirm(s)}
              >
                Confirm
              </button>
              <button
                type="button"
                class="secondary"
                aria-label="Dismiss {s.name}"
                disabled={busy}
                onclick={() => dismiss(s)}
              >
                Dismiss
              </button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .summary {
    display: grid;
    grid-template-columns: minmax(12rem, 1fr) 2fr;
    gap: 2rem;
    align-items: start;
  }

  dl {
    margin: 0;
    display: grid;
    gap: 0.375rem;
  }

  dl div {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
  }

  dt {
    color: var(--text-muted);
  }

  dd {
    margin: 0;
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .figure {
    margin: 0.25rem 0 0;
    font-size: 2.25rem;
    font-weight: 600;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }

  .left .track {
    display: block;
    margin-top: 0.75rem;
    height: 0.75rem;
  }

  .detail {
    margin: 0.375rem 0 0;
    font-size: 0.875rem;
    color: var(--text-muted);
  }

  .cue {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--negative);
  }

  p.cue {
    margin: 0.25rem 0 0;
  }

  .overspent .figure {
    color: var(--negative);
  }

  .track {
    display: block;
    height: 0.5rem;
    border-radius: 999px;
    background: var(--track);
    overflow: hidden;
  }

  .fill {
    display: block;
    height: 100%;
    border-radius: 999px;
    background: var(--blue);
  }

  .overspent .fill {
    background: var(--orange);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .rows li {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--border);
  }

  .rows li:last-child {
    border-bottom: none;
  }

  .name {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  .muted {
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .amount {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .status {
    font-size: 0.75rem;
    font-weight: 600;
    padding: 0.125rem 0.5rem;
    border-radius: 999px;
    border: 1px solid currentColor;
    min-width: 5.5rem;
    text-align: center;
  }

  .status.paid {
    color: var(--positive);
  }

  .status.upcoming {
    color: var(--text-muted);
  }

  .status.past_due {
    color: var(--negative);
  }

  .targets {
    display: grid;
    gap: 1rem;
  }

  .target-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1rem;
    margin-bottom: 0.375rem;
  }

  .target-row .name {
    font-weight: 500;
  }

  .spent {
    font-size: 0.875rem;
    font-variant-numeric: tabular-nums;
  }

  .actions,
  .edit,
  .add {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }

  .actions,
  .edit {
    margin-top: 0.5rem;
  }

  .actions button,
  .edit button,
  .add button {
    font-size: 0.8125rem;
    padding: 0.25rem 0.75rem;
  }

  label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  input[type='number'] {
    width: 7rem;
  }

  .add {
    margin-top: 1.25rem;
    padding-top: 1rem;
    border-top: 1px solid var(--border);
  }

  .add h3 {
    flex-basis: 100%;
    margin: 0;
    font-size: 0.875rem;
    font-weight: 600;
  }

  .none {
    color: var(--text-muted);
    margin: 0.25rem 0;
  }
</style>

<script lang="ts">
  import type { AccountView, StatusView } from './types'
  import { accountTypeLabel, formatMoney } from './format'

  let {
    title,
    subtotal,
    accounts,
  }: { title: string; subtotal: number; accounts: AccountView[] } = $props()

  const headingId = $derived(`group-${title.toLowerCase()}`)

  // Status is always text plus an icon, with colour only reinforcing it
  // (FR9, FR27). `unknown` is D14's forward-compat fallback, kept distinct
  // from `error` so a working source isn't reported as broken.
  const statusLabel: Record<StatusView, { icon: string; text: string }> = {
    ok: { icon: '✓', text: 'OK' },
    error: { icon: '✕', text: 'Failed' },
    unknown: { icon: '?', text: 'Unknown status' },
  }

  // One colour per asset class; each bar is also labelled, so the colour
  // is never the only way to tell them apart.
  const holdingClass: Record<string, string> = {
    Cash: 'cash',
    Fund: 'fund',
    'Individual Stock': 'stock',
  }
</script>

<section class="group" aria-labelledby={headingId}>
  <header>
    <h2 id={headingId}>{title}</h2>
    <!-- Over status-OK balances only, like the TUI's panel titles. -->
    <span class="subtotal">{formatMoney(subtotal)}</span>
  </header>

  {#if accounts.length === 0}
    <p class="none">None configured</p>
  {:else}
    <ul>
      {#each accounts as account (account.source_id)}
        <li class="account {account.status}">
          <div class="row">
            <div class="name">
              <span class="institution">{account.institution}</span>
              <span class="type">{accountTypeLabel(account.account_type)}</span>
            </div>
            <div class="figures">
              {#if account.status === 'ok'}
                <span class="balance">
                  {account.balance === null ? '—' : formatMoney(account.balance, account.currency)}
                </span>
              {/if}
              <span class="status {account.status}">
                <span aria-hidden="true">{statusLabel[account.status].icon}</span>
                {statusLabel[account.status].text}
              </span>
            </div>
          </div>

          {#if account.error_message}
            <!-- FR18: a failed source says why, rather than leaving a blank. -->
            <p class="error">{account.error_message}</p>
          {/if}

          {#if account.holdings && account.holdings.length > 0}
            <ul class="holdings" aria-label="{account.institution} holdings by asset class">
              {#each account.holdings as bucket (bucket.label)}
                <li>
                  <span class="holding-label">{bucket.label}</span>
                  <span class="track" aria-hidden="true">
                    <span
                      class="fill {holdingClass[bucket.label] ?? 'other'}"
                      style:width="{bucket.percentage}%"
                    ></span>
                  </span>
                  <span class="holding-figures">
                    {formatMoney(bucket.value, account.currency)}
                    <span class="percentage">{bucket.percentage.toFixed(1)}%</span>
                  </span>
                </li>
              {/each}
            </ul>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .group {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1rem 1.25rem 0.5rem;
  }

  header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .subtotal {
    font-size: 1.125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .account {
    padding: 0.875rem 0;
    border-bottom: 1px solid var(--border);
  }

  .account:last-child {
    border-bottom: none;
  }

  .row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1rem;
  }

  .name {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .institution {
    font-weight: 500;
  }

  .type {
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .figures {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    white-space: nowrap;
  }

  .balance {
    font-variant-numeric: tabular-nums;
    font-weight: 500;
  }

  .status {
    font-size: 0.75rem;
    font-weight: 600;
    padding: 0.125rem 0.5rem;
    border-radius: 999px;
    border: 1px solid currentColor;
  }

  .status.ok {
    color: var(--positive);
  }

  .status.error {
    color: var(--negative);
  }

  .status.unknown {
    color: var(--caution);
  }

  .error {
    margin: 0.375rem 0 0;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .holdings {
    margin-top: 0.75rem;
    display: grid;
    gap: 0.375rem;
  }

  .holdings li {
    display: grid;
    grid-template-columns: 7.5rem 1fr auto;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.8125rem;
  }

  .holding-label {
    color: var(--text-muted);
  }

  .track {
    height: 0.5rem;
    border-radius: 999px;
    background: var(--track);
    overflow: hidden;
  }

  .fill {
    display: block;
    height: 100%;
    border-radius: 999px;
  }

  .fill.cash {
    background: var(--sky-blue);
  }

  .fill.fund {
    background: var(--blue);
  }

  .fill.stock {
    background: var(--orange);
  }

  .fill.other {
    background: var(--reddish-purple);
  }

  .holding-figures {
    font-variant-numeric: tabular-nums;
    text-align: right;
    min-width: 8.5rem;
  }

  .percentage {
    display: inline-block;
    min-width: 3.25rem;
    color: var(--text-muted);
  }

  .none {
    color: var(--text-muted);
    margin: 0.875rem 0;
  }
</style>

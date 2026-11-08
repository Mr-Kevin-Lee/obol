<script lang="ts">
  import type { AccountView, StatusView } from './types'
  import { accountTypeLabel, formatMoney } from './format'

  let {
    title,
    subtotal,
    accounts,
    onrename,
  }: {
    title: string
    subtotal: number
    accounts: AccountView[]
    // Saves a label (blank clears it); rejects with why it couldn't.
    onrename: (sourceId: string, label: string) => Promise<void>
  } = $props()

  const headingId = $derived(`group-${title.toLowerCase()}`)

  // Your own name tells two accounts at one bank apart (spec: account labels).
  const maxLabel = 60
  let editing = $state<string | null>(null)
  let draft = $state('')
  let saving = $state(false)
  let editError = $state<string | null>(null)
  let input = $state<HTMLInputElement | null>(null)

  $effect(() => {
    input?.focus()
    input?.select()
  })

  function startEdit(account: AccountView) {
    editing = account.source_id
    draft = account.label ?? ''
    editError = null
  }

  async function save(account: AccountView) {
    saving = true
    editError = null
    try {
      await onrename(account.source_id, draft)
      editing = null
    } catch (e) {
      editError = `Couldn't save the name: ${e}`
    } finally {
      saving = false
    }
  }

  function onkeydown(event: KeyboardEvent, account: AccountView) {
    if (event.key === 'Enter') save(account)
    else if (event.key === 'Escape') editing = null
  }

  // Status is always text plus an icon, with colour only reinforcing it
  // (FR9, FR27). `unknown` is D14's forward-compat fallback, kept distinct
  // from `error` so a working source isn't reported as broken.
  const statusLabel: Record<StatusView, { icon: string; text: string }> = {
    ok: { icon: '✓', text: 'OK' },
    error: { icon: '✕', text: 'Failed' },
    unknown: { icon: '?', text: 'Unknown status' },
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
              {#if editing === account.source_id}
                <div class="edit">
                  <input
                    bind:this={input}
                    bind:value={draft}
                    maxlength={maxLabel}
                    placeholder={account.institution}
                    aria-label="Name for this {account.institution} account"
                    disabled={saving}
                    onkeydown={(e) => onkeydown(e, account)}
                  />
                  <button type="button" onclick={() => save(account)} disabled={saving}>
                    Save
                  </button>
                  <button
                    type="button"
                    class="secondary"
                    onclick={() => (editing = null)}
                    disabled={saving}
                  >
                    Cancel
                  </button>
                </div>
              {:else}
                <span class="institution">
                  {account.label ?? account.institution}
                  <button
                    type="button"
                    class="secondary rename"
                    aria-label={account.label
                      ? `Rename ${account.label}`
                      : `Name the ${account.institution} ${accountTypeLabel(account.account_type)} account${account.balance === null ? '' : ` with balance ${formatMoney(account.balance, account.currency)}`}`}
                    onclick={() => startEdit(account)}
                  >
                    {account.label ? 'Rename' : 'Name it'}
                  </button>
                </span>
              {/if}
              <span class="type">
                {account.label ? `${account.institution} · ` : ''}{accountTypeLabel(
                  account.account_type,
                )}
              </span>
              {#if editing === account.source_id && editError}
                <span class="edit-error" role="alert">{editError}</span>
              {/if}
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
    font-size: 1.25rem;
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

  /* Shown on hover or keyboard focus, so a quiet list stays quiet. */
  .rename {
    margin-left: 0.5rem;
    padding: 0 0.5rem;
    font-size: 0.75rem;
    font-weight: 400;
    opacity: 0;
  }

  li:hover .rename,
  .rename:focus-visible {
    opacity: 1;
  }

  @media (hover: none) {
    .rename {
      opacity: 1;
    }
  }

  .edit {
    display: flex;
    gap: 0.375rem;
    align-items: center;
  }

  .edit input {
    min-width: 0;
    flex: 1;
  }

  .edit button {
    padding: 0.2rem 0.6rem;
    font-size: 0.8125rem;
  }

  .edit-error {
    font-size: 0.8125rem;
    color: var(--negative);
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

  .none {
    color: var(--text-muted);
    margin: 0.875rem 0;
  }
</style>

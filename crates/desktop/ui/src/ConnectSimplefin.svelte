<script lang="ts">
  import { tick } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import { accountTypeLabel, formatMoney } from './format'
  import type { ConnectAccount, ConnectPreview, LinkResult, Side } from './types'

  let {
    reuse = false,
    onlinked,
    oncancel,
  }: {
    /** Opened from "Add accounts": the saved connection is the main action. */
    reuse?: boolean
    onlinked: (linked: number) => void
    oncancel?: () => void
  } = $props()

  const createUrl = 'https://bridge.simplefin.org/simplefin/create'

  const accountTypes = [
    'checking',
    'savings',
    'money_market',
    'credit_card',
    'loan',
    'investment',
    'other',
  ]
  const sideOf: Record<string, Side> = {
    checking: 'asset',
    savings: 'asset',
    money_market: 'asset',
    investment: 'asset',
    credit_card: 'liability',
    loan: 'liability',
  }

  let token = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)
  let copied = $state<string | null>(null)
  let preview = $state<ConnectPreview | null>(null)
  let tableHeading = $state<HTMLElement | null>(null)

  const included = $derived(preview ? preview.accounts.filter((a) => a.include).length : 0)
  // Nothing ticked still saves: the unticked rows are remembered as ignored.
  const linkLabel = $derived(
    included === 0 ? 'Save' : `Link ${included} ${included === 1 ? 'account' : 'accounts'}`,
  )

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(createUrl)
      copied = 'Link copied.'
    } catch {
      copied = "Couldn't copy. Select the link and copy it instead."
    }
  }

  async function connect(setupToken: string | null) {
    busy = true
    error = null
    try {
      preview = await invoke<ConnectPreview>('preview_simplefin', { setupToken })
      // Claiming spends the token, so it must never be sent again.
      token = ''
      await tick()
      tableHeading?.focus()
    } catch (e) {
      error = `Couldn't connect to SimpleFIN: ${e}`
      // A claimed token is useless; anything else is worth retrying as is.
      if (/claimed|already used|spent|403|forbidden/i.test(String(e))) token = ''
    } finally {
      busy = false
    }
  }

  async function link() {
    if (!preview) return
    busy = true
    error = null
    try {
      // Unticked rows go too, so the backend remembers them as ignored.
      const result = await invoke<LinkResult>('link_simplefin_accounts', {
        accounts: preview.accounts,
      })
      preview = null
      onlinked(result.linked)
    } catch (e) {
      error = `Couldn't link the accounts: ${e}`
    } finally {
      busy = false
    }
  }

  // A credit card can't be an asset, nor checking a liability.
  function setSide(row: ConnectAccount, side: Side) {
    row.side = side
    const natural = sideOf[row.account_type]
    if (natural && natural !== side) row.account_type = 'other'
  }

  function cancel() {
    preview = null
    error = null
    oncancel?.()
  }

  function balanceText(balance: string) {
    const n = Number(balance)
    return balance.trim() !== '' && Number.isFinite(n) ? formatMoney(n) : balance
  }

  const label = (a: ConnectAccount) => `${a.institution} ${a.name}`
</script>

<section class="card connect" aria-labelledby="connect-heading">
  {#if preview === null}
    <h2 id="connect-heading">{reuse ? 'Add accounts' : 'Connect your bank accounts'}</h2>
    <p>
      Obol reads your balances through SimpleFIN Bridge. Create a Setup Token there, then paste it
      below.
    </p>
    <div class="link-row">
      <code class="url">{createUrl}</code>
      <button type="button" class="secondary" onclick={copyLink}>Copy link</button>
      {#if copied}<span class="muted" role="status">{copied}</span>{/if}
    </div>

    {#if error}
      <div class="banner error" role="alert">{error}</div>
    {/if}

    <form
      class="token"
      onsubmit={(e) => {
        e.preventDefault()
        if (token.trim() !== '') connect(token.trim())
      }}
    >
      <label>
        Setup Token
        <input
          type="password"
          autocomplete="off"
          spellcheck="false"
          disabled={busy}
          bind:value={token}
        />
      </label>
      <button
        type="submit"
        class:secondary={reuse}
        disabled={busy || token.trim() === ''}
        aria-busy={busy}
      >
        {busy ? 'Connecting…' : 'Continue'}
      </button>
      <button
        type="button"
        class:secondary={!reuse}
        disabled={busy}
        onclick={() => connect(null)}
      >
        Use my saved connection
      </button>
      {#if oncancel}
        <button type="button" class="secondary" disabled={busy} onclick={cancel}>Cancel</button>
      {/if}
    </form>
  {:else}
    <h2 id="connect-heading" tabindex="-1" bind:this={tableHeading}>Choose accounts to link</h2>

    {#if preview.warnings.length > 0}
      <div class="banner warning" role="status">
        <p>SimpleFIN reported:</p>
        <ul>
          {#each preview.warnings as warning, i (i)}
            <li>{warning}</li>
          {/each}
        </ul>
      </div>
    {/if}

    {#if error}
      <div class="banner error" role="alert">{error}</div>
    {/if}

    {#if preview.accounts.length === 0}
      <p class="none">All your SimpleFIN accounts are already connected.</p>
    {:else}
      <table>
        <thead>
          <tr>
            <th scope="col">Include</th>
            <th scope="col">Account</th>
            <th scope="col" class="amount">Balance</th>
            <th scope="col">Side</th>
            <th scope="col">Type</th>
          </tr>
        </thead>
        <tbody>
          {#each preview.accounts as row (row.account_id)}
            <tr>
              <td>
                <input
                  type="checkbox"
                  aria-label="Include {label(row)}"
                  disabled={busy}
                  bind:checked={row.include}
                />
              </td>
              <td>
                <span class="muted">{row.institution}</span>
                {row.name}
              </td>
              <td class="amount">{balanceText(row.balance)}</td>
              <td>
                <select
                  aria-label="Side for {label(row)}"
                  disabled={busy}
                  value={row.side}
                  onchange={(e) => setSide(row, e.currentTarget.value as Side)}
                >
                  <option value="asset">Asset</option>
                  <option value="liability">Liability</option>
                </select>
              </td>
              <td>
                <select
                  aria-label="Type for {label(row)}"
                  disabled={busy}
                  bind:value={row.account_type}
                >
                  {#each accountTypes as t (t)}
                    <option value={t}>{accountTypeLabel(t)}</option>
                  {/each}
                </select>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}

    <div class="actions">
      {#if preview.accounts.length > 0}
        <button type="button" disabled={busy} aria-busy={busy} onclick={link}>
          {busy ? 'Linking…' : linkLabel}
        </button>
      {/if}
      <button type="button" class="secondary" disabled={busy} onclick={cancel}>
        {preview.accounts.length === 0 ? 'Close' : 'Cancel'}
      </button>
    </div>
  {/if}
</section>

<style>
  .connect p {
    margin: 0 0 0.75rem;
  }

  .link-row,
  .token,
  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
  }

  .url {
    user-select: all;
    -webkit-user-select: all;
    padding: 0.25rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .token,
  .actions {
    margin-top: 1rem;
  }

  .link-row button,
  .token button,
  .actions button {
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

  .token input {
    width: 20rem;
  }

  h2:focus {
    outline: none;
  }

  .banner p {
    margin: 0;
  }

  .banner ul {
    margin: 0.25rem 0 0;
    padding-left: 1.25rem;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875rem;
    margin-top: 0.75rem;
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

  .muted {
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .amount {
    text-align: right;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .none {
    color: var(--text-muted);
  }
</style>

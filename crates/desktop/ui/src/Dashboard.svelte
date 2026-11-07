<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import AccountGroup from './AccountGroup.svelte'
  import ConnectSimplefin from './ConnectSimplefin.svelte'
  import NetWorthHeader from './NetWorthHeader.svelte'
  import { insideApp, notInAppMessage } from './backend'
  import { formatDateTime } from './format'
  import type { ConnectStatus, DashboardView, RefreshResult } from './types'

  // Lets the header coin spin while a refresh runs.
  let { refreshing = $bindable(false) }: { refreshing?: boolean } = $props()

  let view = $state<DashboardView | null>(null)
  let loading = $state(true)
  let error = $state<string | null>(null)
  let saveWarning = $state<string | null>(null)
  let syncWarning = $state<string | null>(null)

  // null until known (or if the check fails): then only "Add accounts" shows.
  let simplefinSources = $state<number | null>(null)
  let adding = $state(false)
  let linked = $state<string | null>(null)
  let addedSources = $state<string[]>([])

  const inApp = insideApp()
  const accounts = (n: number) => `${n} ${n === 1 ? 'account' : 'accounts'}`

  async function loadStatus() {
    try {
      simplefinSources = (await invoke<ConnectStatus>('connect_status')).simplefin_sources
    } catch {
      // Not worth a banner: the form is still reachable from "Add accounts".
      simplefinSources = null
    }
  }

  onMount(async () => {
    if (!inApp) {
      error = notInAppMessage
      loading = false
      return
    }
    loadStatus()
    try {
      // Disk only: the window opens on the last snapshot instead of
      // waiting on every provider (D43, superseding D19).
      view = await invoke<DashboardView | null>('get_dashboard')
    } catch (e) {
      error = `Couldn't load your dashboard: ${e}`
    } finally {
      loading = false
    }
  })

  async function refresh() {
    refreshing = true
    error = null
    saveWarning = null
    syncWarning = null
    try {
      const result = await invoke<RefreshResult>('refresh')
      view = result.view
      // §9.1: a failed save never hides the fetch, it just gets flagged.
      saveWarning = result.save_warning
      syncWarning = result.sync_warning
      // Kept until dismissed, so a later quiet refresh doesn't hide it.
      if (result.added_sources.length > 0) addedSources = result.added_sources
    } catch (e) {
      // The last good view stays on screen; the banner says why it's stale.
      error = `Refresh failed: ${e}`
    } finally {
      refreshing = false
    }
  }

  function openForm() {
    linked = null
    adding = true
  }

  async function onLinked(n: number) {
    adding = false
    linked = `Linked ${accounts(n)}.`
    await loadStatus()
    if (n > 0) await refresh()
  }
</script>

<div class="toolbar">
  <p class="as-of">
    {#if view}As of {formatDateTime(view.created_at)}{/if}
  </p>
  <div class="toolbar-actions">
    {#if inApp && simplefinSources !== 0 && !adding}
      <button type="button" class="secondary" onclick={openForm} disabled={loading || refreshing}>
        Add accounts
      </button>
    {/if}
    <button
      type="button"
      onclick={refresh}
      disabled={!inApp || loading || refreshing}
      aria-busy={refreshing}
    >
      {refreshing ? 'Refreshing…' : 'Refresh'}
    </button>
  </div>
</div>

{#if error}
  <div class="banner error" role="alert">{error}</div>
{/if}
{#if saveWarning}
  <div class="banner warning" role="status">{saveWarning}</div>
{/if}
{#if syncWarning}
  <div class="banner warning" role="status">{syncWarning}</div>
{/if}
{#if linked}
  <div class="banner notice" role="status">{linked}</div>
{/if}
{#if addedSources.length > 0}
  <div class="banner notice dismissible" role="status">
    <span>
      Added {addedSources.length} new {addedSources.length === 1 ? 'account' : 'accounts'}:
      {addedSources.join(', ')}.
    </span>
    <button type="button" class="secondary" onclick={() => (addedSources = [])}>Dismiss</button>
  </div>
{/if}

{#if inApp && !loading && (simplefinSources === 0 || adding)}
  <ConnectSimplefin
    reuse={simplefinSources !== 0}
    onlinked={onLinked}
    oncancel={simplefinSources === 0 ? undefined : () => (adding = false)}
  />
{/if}

{#if loading}
  <p class="placeholder">Loading…</p>
{:else if view}
  <NetWorthHeader netWorth={view.net_worth} delta={view.delta} trend={view.trend} />
  <div class="groups">
    <AccountGroup title="Assets" subtotal={view.assets_subtotal} accounts={view.assets} />
    <AccountGroup
      title="Liabilities"
      subtotal={view.liabilities_subtotal}
      accounts={view.liabilities}
    />
  </div>
{:else if inApp && !error && simplefinSources !== 0}
  <div class="empty">
    <h2>No snapshots yet</h2>
    <p>Press Refresh to fetch your first one.</p>
  </div>
{/if}

<style>
  .toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    padding-top: 1rem;
  }

  .toolbar-actions {
    display: flex;
    gap: 0.5rem;
  }

  .notice {
    border-color: var(--positive);
  }

  .dismissible {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
  }

  .dismissible button {
    font-size: 0.8125rem;
    padding: 0.25rem 0.75rem;
  }

  .as-of {
    margin: 0;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
    gap: 1.25rem;
    align-items: start;
  }
</style>

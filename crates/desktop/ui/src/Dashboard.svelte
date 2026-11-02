<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import AccountGroup from './AccountGroup.svelte'
  import NetWorthHeader from './NetWorthHeader.svelte'
  import { insideApp, notInAppMessage } from './backend'
  import { formatDateTime } from './format'
  import type { DashboardView, RefreshResult } from './types'

  let view = $state<DashboardView | null>(null)
  let loading = $state(true)
  let refreshing = $state(false)
  let error = $state<string | null>(null)
  let saveWarning = $state<string | null>(null)

  const inApp = insideApp()

  onMount(async () => {
    if (!inApp) {
      error = notInAppMessage
      loading = false
      return
    }
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
    try {
      const result = await invoke<RefreshResult>('refresh')
      view = result.view
      // §9.1: a failed save never hides the fetch, it just gets flagged.
      saveWarning = result.save_warning
    } catch (e) {
      // The last good view stays on screen; the banner says why it's stale.
      error = `Refresh failed: ${e}`
    } finally {
      refreshing = false
    }
  }
</script>

<div class="toolbar">
  <p class="as-of">
    {#if view}As of {formatDateTime(view.created_at)}{/if}
  </p>
  <button
    type="button"
    onclick={refresh}
    disabled={!inApp || loading || refreshing}
    aria-busy={refreshing}
  >
    {refreshing ? 'Refreshing…' : 'Refresh'}
  </button>
</div>

{#if error}
  <div class="banner error" role="alert">{error}</div>
{/if}
{#if saveWarning}
  <div class="banner warning" role="status">{saveWarning}</div>
{/if}

{#if loading}
  <p class="placeholder">Loading…</p>
{:else if view}
  <NetWorthHeader netWorth={view.net_worth} delta={view.delta} />
  <div class="groups">
    <AccountGroup title="Assets" subtotal={view.assets_subtotal} accounts={view.assets} />
    <AccountGroup
      title="Liabilities"
      subtotal={view.liabilities_subtotal}
      accounts={view.liabilities}
    />
  </div>
{:else if inApp && !error}
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

<script lang="ts">
  import { onMount } from 'svelte'
  import { invoke } from '@tauri-apps/api/core'
  import AccountGroup from './AccountGroup.svelte'
  import NetWorthHeader from './NetWorthHeader.svelte'
  import { formatDateTime } from './format'
  import type { DashboardView, RefreshResult } from './types'

  let view = $state<DashboardView | null>(null)
  let loading = $state(true)
  let refreshing = $state(false)
  let error = $state<string | null>(null)
  let saveWarning = $state<string | null>(null)

  // Opened in a plain browser (e.g. Vite's dev URL) there's no backend to
  // call; say so rather than surfacing an opaque invoke error.
  const insideApp = '__TAURI_INTERNALS__' in window

  onMount(async () => {
    if (!insideApp) {
      error = 'Not running inside the Obol app. Launch it with `make desktop-dev`.'
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

<main>
  <header class="top">
    <div>
      <h1>Obol</h1>
      {#if view}
        <p class="as-of">As of {formatDateTime(view.created_at)}</p>
      {/if}
    </div>
    <button
      type="button"
      onclick={refresh}
      disabled={!insideApp || loading || refreshing}
      aria-busy={refreshing}
    >
      {refreshing ? 'Refreshing…' : 'Refresh'}
    </button>
  </header>

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
  {:else if insideApp && !error}
    <div class="empty">
      <h2>No snapshots yet</h2>
      <p>Press Refresh to fetch your first one.</p>
    </div>
  {/if}
</main>

<style>
  /* Okabe–Ito (spec §14). Fills use the palette's exact values. Text uses
     darker (light mode) or lighter (dark mode) shades of the same hues,
     because bluish green and vermillion as small text miss WCAG AA's
     4.5:1 contrast at their published values. Hue is what keeps them
     distinguishable under colour-vision deficiency, so that's unchanged. */
  :global(:root) {
    --blue: #0072b2;
    --sky-blue: #56b4e9;
    --orange: #e69f00;
    --reddish-purple: #cc79a7;

    --positive: #00785a;
    --negative: #b34e00;
    --caution: #9a6a00;

    --bg: #f5f5f7;
    --surface: #ffffff;
    --border: #d2d2d7;
    --track: #e8e8ed;
    --text: #1d1d1f;
    --text-muted: #6e6e73;

    color-scheme: light dark;
  }

  @media (prefers-color-scheme: dark) {
    :global(:root) {
      --positive: #2bb58c;
      --negative: #f07a30;
      --caution: #e69f00;

      --bg: #1c1c1e;
      --surface: #2c2c2e;
      --border: #3a3a3c;
      --track: #3a3a3c;
      --text: #f5f5f7;
      --text-muted: #a1a1a6;
    }
  }

  :global(body) {
    margin: 0;
    background: var(--bg);
    color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, 'Helvetica Neue', sans-serif;
    font-size: 15px;
    line-height: 1.4;
    -webkit-font-smoothing: antialiased;
  }

  :global(code) {
    font-family: ui-monospace, 'SF Mono', Menlo, monospace;
    font-size: 0.9em;
  }

  :global(.eyebrow) {
    margin: 0;
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-muted);
  }

  main {
    max-width: 1040px;
    margin: 0 auto;
    padding: 1.5rem 2rem 3rem;
  }

  .top {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  h1 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  .as-of {
    margin: 0.125rem 0 0;
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  button {
    font: inherit;
    font-weight: 500;
    padding: 0.4rem 1rem;
    border-radius: 8px;
    border: 1px solid var(--blue);
    background: var(--blue);
    color: #ffffff;
    cursor: pointer;
  }

  button:disabled {
    opacity: 0.55;
    cursor: default;
  }

  button:focus-visible {
    outline: 2px solid var(--sky-blue);
    outline-offset: 2px;
  }

  .banner {
    margin-top: 1rem;
    padding: 0.75rem 1rem;
    border-radius: 8px;
    border: 1px solid;
    background: var(--surface);
  }

  .banner.error {
    border-color: var(--negative);
    color: var(--negative);
  }

  .banner.warning {
    border-color: var(--caution);
    color: var(--caution);
  }

  .groups {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
    gap: 1.25rem;
    align-items: start;
  }

  .placeholder,
  .empty {
    color: var(--text-muted);
    text-align: center;
    padding: 4rem 0;
  }

  .empty h2 {
    margin: 0;
    font-size: 1.25rem;
    color: var(--text);
  }

  .empty p {
    margin: 0.5rem 0 0;
  }
</style>

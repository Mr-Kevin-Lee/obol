<script lang="ts">
  import Dashboard from './Dashboard.svelte'
  import MonthSelector from './MonthSelector.svelte'
  import Transactions from './Transactions.svelte'
  import { currentMonth } from './calendar'

  type Screen = 'net-worth' | 'transactions' | 'plan'
  const screens: { id: Screen; label: string }[] = [
    { id: 'net-worth', label: 'Net worth' },
    { id: 'transactions', label: 'Transactions' },
    { id: 'plan', label: 'Spending plan' },
  ]

  let screen = $state<Screen>('net-worth')
  // Shared by both budget screens, so switching between them keeps your place.
  let month = $state(currentMonth())
</script>

<main>
  <header class="top">
    <h1>Obol</h1>
    <nav aria-label="Screens">
      {#each screens as s (s.id)}
        <button
          type="button"
          class="tab"
          aria-current={screen === s.id ? 'page' : undefined}
          onclick={() => (screen = s.id)}
        >
          {s.label}
        </button>
      {/each}
    </nav>
  </header>

  <!-- Hidden rather than unmounted, so a refresh in flight survives a tab switch. -->
  <div hidden={screen !== 'net-worth'}>
    <Dashboard />
  </div>

  {#if screen !== 'net-worth'}
    <MonthSelector bind:month />
    {#if screen === 'transactions'}
      <Transactions {month} />
    {:else}
      <p class="placeholder">The spending plan arrives in task 119.</p>
    {/if}
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
    --link: #0072b2;

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
      --link: #56b4e9;

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

  /* Shared by every screen. */
  :global(button) {
    font: inherit;
    font-weight: 500;
    padding: 0.4rem 1rem;
    border-radius: 8px;
    border: 1px solid var(--blue);
    background: var(--blue);
    color: #ffffff;
    cursor: pointer;
  }

  :global(button.secondary) {
    background: transparent;
    color: var(--link);
    border-color: var(--border);
  }

  :global(button:disabled) {
    opacity: 0.55;
    cursor: default;
  }

  :global(button:focus-visible),
  :global(select:focus-visible),
  :global(input:focus-visible) {
    outline: 2px solid var(--sky-blue);
    outline-offset: 2px;
  }

  :global(select),
  :global(input) {
    font: inherit;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.25rem 0.4rem;
  }

  :global(.banner) {
    margin-top: 1rem;
    padding: 0.75rem 1rem;
    border-radius: 8px;
    border: 1px solid;
    background: var(--surface);
  }

  :global(.banner.error) {
    border-color: var(--negative);
    color: var(--negative);
  }

  :global(.banner.warning) {
    border-color: var(--caution);
    color: var(--caution);
  }

  :global(.placeholder),
  :global(.empty) {
    color: var(--text-muted);
    text-align: center;
    padding: 4rem 0;
  }

  :global(.empty h2) {
    margin: 0;
    font-size: 1.25rem;
    color: var(--text);
  }

  :global(.empty p) {
    margin: 0.5rem 0 0;
  }

  :global(.card) {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 1rem 1.25rem;
    margin-top: 1.25rem;
  }

  :global(.card > h2) {
    margin: 0 0 0.75rem;
    font-size: 1rem;
    font-weight: 600;
  }

  main {
    max-width: 1040px;
    margin: 0 auto;
    padding: 1.5rem 2rem 3rem;
  }

  .top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  h1 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  nav {
    display: flex;
    gap: 0.25rem;
  }

  .tab {
    background: transparent;
    border-color: transparent;
    color: var(--text-muted);
  }

  /* The current tab is marked by weight and an underline as well as colour. */
  .tab[aria-current='page'] {
    color: var(--text);
    font-weight: 600;
    border-color: var(--border);
    background: var(--surface);
    text-decoration: underline;
    text-underline-offset: 0.3em;
  }
</style>

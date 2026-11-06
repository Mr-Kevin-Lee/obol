<script lang="ts">
  import type { CategoryBar } from './types'
  import { barWidth, formatMoney, formatPercentage } from './format'

  // With `onselect`, each bar is a toggle button: the Transactions screen
  // filters by it. Without, it's a plain list (the Spending plan).
  let {
    bars,
    label,
    selected = null,
    onselect,
  }: {
    bars: CategoryBar[]
    label: string
    selected?: string | null
    onselect?: (category: string) => void
  } = $props()
</script>

{#snippet bar_contents(bar: CategoryBar)}
  <span class="name">{bar.category}</span>
  <span class="track" aria-hidden="true">
    <span class="fill" style:width={barWidth(bar.percentage)}></span>
  </span>
  <span class="figures">
    {formatMoney(bar.spent)}
    <span class="percentage">{formatPercentage(bar.percentage)}</span>
  </span>
{/snippet}

<!-- Every bar carries its amount and percentage in text, so the length is
     never the only cue. -->
<ul aria-label={label}>
  {#each bars as bar (bar.category)}
    <li>
      {#if onselect}
        <button
          type="button"
          class="row"
          class:selected={selected === bar.category}
          aria-pressed={selected === bar.category}
          title={selected === bar.category ? 'Show every category' : `Show only ${bar.category}`}
          onclick={() => onselect(bar.category)}
        >
          {@render bar_contents(bar)}
        </button>
      {:else}
        <div class="row">{@render bar_contents(bar)}</div>
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.25rem;
  }

  .row {
    display: grid;
    grid-template-columns: 9rem 1fr auto;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.875rem;
    padding: 0.125rem 0.25rem;
  }

  button.row {
    width: 100%;
    border: 1px solid transparent;
    border-radius: 6px;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 0.875rem;
    text-align: left;
    cursor: pointer;
  }

  button.row:hover {
    background: var(--track);
  }

  /* Selected is shown by weight and an outline, not colour alone. */
  button.row.selected {
    border-color: var(--blue);
    font-weight: 600;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    background: var(--blue);
  }

  .figures {
    font-variant-numeric: tabular-nums;
    text-align: right;
    min-width: 9rem;
  }

  .percentage {
    display: inline-block;
    min-width: 3.5rem;
    color: var(--text-muted);
  }
</style>

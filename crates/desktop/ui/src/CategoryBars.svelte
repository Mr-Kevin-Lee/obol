<script lang="ts">
  import type { CategoryBar } from './types'
  import { barWidth, formatMoney, formatPercentage } from './format'

  let { bars, label }: { bars: CategoryBar[]; label: string } = $props()
</script>

<!-- Same bar style as the dashboard's holdings; every bar carries its
     amount and percentage in text, so the length is never the only cue. -->
<ul aria-label={label}>
  {#each bars as bar (bar.category)}
    <li>
      <span class="name">{bar.category}</span>
      <span class="track" aria-hidden="true">
        <span class="fill" style:width={barWidth(bar.percentage)}></span>
      </span>
      <span class="figures">
        {formatMoney(bar.spent)}
        <span class="percentage">{formatPercentage(bar.percentage)}</span>
      </span>
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.5rem;
  }

  li {
    display: grid;
    grid-template-columns: 9rem 1fr auto;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.875rem;
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

<script lang="ts">
  import { shiftMonth } from './calendar'
  import { formatMonth } from './format'

  let { month = $bindable() }: { month: string } = $props()
</script>

<div class="month" role="group" aria-label="Month">
  <button
    type="button"
    class="secondary"
    aria-label="Previous month"
    onclick={() => (month = shiftMonth(month, -1))}
  >
    <span aria-hidden="true">‹</span>
  </button>
  <h2 aria-live="polite">{formatMonth(month)}</h2>
  <button
    type="button"
    class="secondary"
    aria-label="Next month"
    onclick={() => (month = shiftMonth(month, 1))}
  >
    <span aria-hidden="true">›</span>
  </button>
</div>

<style>
  .month {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding-top: 1.25rem;
  }

  h2 {
    margin: 0;
    min-width: 10rem;
    text-align: center;
    font-size: 1.125rem;
    font-weight: 600;
  }

  button {
    padding: 0.2rem 0.7rem;
    font-size: 1.125rem;
    line-height: 1.2;
  }
</style>

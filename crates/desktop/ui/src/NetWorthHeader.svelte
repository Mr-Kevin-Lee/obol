<script lang="ts">
  import type { DeltaView, NetWorthView } from './types'
  import { formatDate, formatMoney, formatSignedMoney } from './format'

  let { netWorth, delta }: { netWorth: NetWorthView; delta: DeltaView | null } = $props()

  // Direction is carried by an arrow and a sign as well as by colour
  // (FR9, FR27), so it reads correctly under any colour vision.
  const direction = $derived(
    delta === null ? null : delta.amount > 0 ? 'up' : delta.amount < 0 ? 'down' : 'flat',
  )
  const arrow = { up: '▲', down: '▼', flat: '■' }
</script>

<section class="net-worth" aria-labelledby="net-worth-heading">
  <h2 id="net-worth-heading" class="eyebrow">Net worth</h2>

  {#if netWorth.kind === 'computed'}
    <p class="figure">{formatMoney(netWorth.value)}</p>
    {#if delta && direction}
      <p class="delta {direction}">
        <span aria-hidden="true">{arrow[direction]}</span>
        {#if direction === 'flat'}
          No change since {formatDate(delta.since)}
        {:else}
          {formatSignedMoney(delta.amount)} since {formatDate(delta.since)}
        {/if}
      </p>
    {/if}
  {:else if netWorth.total_sources === 0}
    <!-- Not the same thing as every source failing: there's nothing
         configured to have failed. -->
    <p class="figure unavailable">No sources configured</p>
    <p class="detail">
      Add one with <code>obol sources</code> in a terminal. Managing sources from this
      window comes in a later release.
    </p>
  {:else}
    <!-- §9.1: never a numeric $0, which would be indistinguishable from a
         genuine zero net worth. -->
    <p class="figure unavailable">Net worth unavailable</p>
    <p class="detail">0/{netWorth.total_sources} sources returned data this run.</p>
  {/if}
</section>

<style>
  .net-worth {
    padding: 1.5rem 0 1.75rem;
  }

  .figure {
    margin: 0.25rem 0 0;
    font-size: 2.75rem;
    font-weight: 600;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }

  .figure.unavailable {
    font-size: 1.75rem;
    color: var(--text-muted);
  }

  .delta {
    margin: 0.5rem 0 0;
    font-weight: 500;
    font-variant-numeric: tabular-nums;
  }

  .delta.up {
    color: var(--positive);
  }

  .delta.down {
    color: var(--negative);
  }

  .delta.flat {
    color: var(--text-muted);
  }

  .detail {
    margin: 0.5rem 0 0;
    color: var(--text-muted);
  }
</style>

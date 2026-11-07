<script lang="ts">
  import { formatMoney } from './format'

  let { values }: { values: number[] } = $props()

  const width = 240
  const height = 48
  const pad = 4

  // Needs two points to be a line; a single snapshot has no trend yet.
  const points = $derived.by(() => {
    if (values.length < 2) return []
    const min = Math.min(...values)
    const range = Math.max(...values) - min
    return values.map((v, i) => ({
      x: pad + (i / (values.length - 1)) * (width - 2 * pad),
      // A flat history sits mid-height instead of dividing by zero.
      y: range === 0 ? height / 2 : height - pad - ((v - min) / range) * (height - 2 * pad),
    }))
  })
  const line = $derived(points.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' '))
  const area = $derived(
    points.length === 0 ? '' : `${pad},${height} ${line} ${width - pad},${height}`,
  )
  const last = $derived(points[points.length - 1])
  const label = $derived(
    values.length < 2
      ? ''
      : `Net worth over the last ${values.length} snapshots, from ${formatMoney(values[0])} to ${formatMoney(values[values.length - 1])}`,
  )
</script>

{#if points.length > 0}
  <svg {width} {height} viewBox="0 0 {width} {height}" role="img" aria-label={label}>
    <polygon points={area} class="area" />
    <polyline points={line} class="line" />
    <circle cx={last.x} cy={last.y} r="3" class="dot" />
  </svg>
{/if}

<style>
  svg {
    display: block;
    margin-top: 0.75rem;
    max-width: 100%;
    overflow: visible;
  }

  .area {
    fill: var(--link);
    opacity: 0.12;
  }

  .line {
    fill: none;
    stroke: var(--link);
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }

  .dot {
    fill: var(--link);
  }
</style>

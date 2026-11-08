<script lang="ts">
  import { formatDate, formatMoney } from './format'
  import type { TrendPoint } from './types'

  let { points }: { points: TrendPoint[] } = $props()

  const height = 96
  const pad = 6
  const dayMs = 86_400_000

  // Measured, so the line, dot and readout stay undistorted at any width.
  let width = $state(0)
  let hovered = $state<number | null>(null)

  // 'YYYY-MM-DD' as a local calendar day, the way formatDate reads it.
  const time = (date: string) => {
    const [y, m, d] = date.split('-').map(Number)
    return new Date(y, m - 1, d).getTime()
  }

  // Needs two points to be a line; a single snapshot has no trend yet.
  const plotted = $derived.by(() => {
    if (points.length < 2 || width === 0) return []
    const t0 = time(points[0].date)
    const span = time(points[points.length - 1].date) - t0
    const values = points.map((p) => p.value)
    const min = Math.min(...values)
    const range = Math.max(...values) - min
    return points.map((p) => ({
      ...p,
      // Placed by date, so a gap in the history is a gap on the chart.
      x: pad + (span === 0 ? 0 : ((time(p.date) - t0) / span) * (width - 2 * pad)),
      // A flat history sits mid-height instead of dividing by zero.
      y: range === 0 ? height / 2 : height - pad - ((p.value - min) / range) * (height - 2 * pad),
    }))
  })
  const line = $derived(plotted.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' '))
  const area = $derived(
    plotted.length === 0
      ? ''
      : `${plotted[0].x},${height} ${line} ${plotted[plotted.length - 1].x},${height}`,
  )
  const active = $derived(plotted[hovered ?? plotted.length - 1])
  const days = $derived(
    points.length < 2
      ? 0
      : Math.round((time(points[points.length - 1].date) - time(points[0].date)) / dayMs),
  )
  const label = $derived(
    points.length < 2
      ? ''
      : `Net worth from ${formatDate(points[0].date)} to ${formatDate(points[points.length - 1].date)}, from ${formatMoney(points[0].value)} to ${formatMoney(points[points.length - 1].value)}. Move over the chart to see each day.`,
  )

  function track(event: PointerEvent) {
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect()
    const x = event.clientX - box.left
    // The point nearest the pointer, by distance along the date axis.
    let best = 0
    plotted.forEach((p, i) => {
      if (Math.abs(p.x - x) < Math.abs(plotted[best].x - x)) best = i
    })
    hovered = best
  }
</script>

{#if points.length >= 2}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="chart"
    bind:clientWidth={width}
    role="img"
    aria-label={label}
    onpointerdown={track}
    onpointermove={track}
    onpointerleave={() => (hovered = null)}
    onpointercancel={() => (hovered = null)}
  >
    {#if plotted.length > 0}
      <svg {width} {height} viewBox="0 0 {width} {height}" aria-hidden="true">
        <polygon points={area} class="area" />
        <polyline points={line} class="line" />
        {#if hovered !== null}
          <line x1={active.x} x2={active.x} y1="0" y2={height} class="guide" />
        {/if}
        <circle cx={active.x} cy={active.y} r="4" class="dot" />
      </svg>
      <p class="readout" aria-hidden="true">
        <strong>{formatMoney(active.value)}</strong>
        <span>{formatDate(active.date)}</span>
      </p>
      <p class="axis" aria-hidden="true">
        <span>{formatDate(points[0].date)}</span>
        <span>{days} days</span>
        <span>{formatDate(points[points.length - 1].date)}</span>
      </p>
    {/if}
  </div>
{/if}

<style>
  .chart {
    margin-top: 1rem;
    max-width: 640px;
    /* Horizontal drags scrub the chart; vertical ones still scroll the page. */
    touch-action: pan-y;
    cursor: crosshair;
  }

  svg {
    display: block;
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

  .guide {
    stroke: var(--text-muted);
    stroke-width: 1;
    stroke-dasharray: 3 3;
  }

  .dot {
    fill: var(--link);
    stroke: var(--surface);
    stroke-width: 2;
  }

  .readout {
    display: flex;
    gap: 0.625rem;
    align-items: baseline;
    margin: 0.5rem 0 0;
    font-variant-numeric: tabular-nums;
  }

  .readout span {
    color: var(--text-muted);
    font-size: 0.8125rem;
  }

  .axis {
    display: flex;
    justify-content: space-between;
    margin: 0.25rem 0 0;
    font-size: 0.75rem;
    color: var(--text-muted);
  }
</style>

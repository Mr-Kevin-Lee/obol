<script lang="ts">
  // The Obol: a small coin with an owl, the one on Athenian silver. Turns
  // once when the pointer touches it (a mouse hover or a finger down), and
  // keeps turning while `spinning` is set, e.g. during a refresh.
  let { spinning = false, size = 32 }: { spinning?: boolean; size?: number } = $props()

  let turning = $state(false)
</script>

<!-- Decorative: the "Obol" heading beside it names the app. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class="coin"
  class:turning
  class:spinning
  style:width="{size}px"
  style:height="{size}px"
  aria-hidden="true"
  onpointerenter={() => (turning = true)}
  onanimationend={() => (turning = false)}
>
  <svg viewBox="0 0 64 64" width={size} height={size}>
    <circle cx="32" cy="32" r="30" class="face" />
    <circle cx="32" cy="32" r="26" class="beads" />
    <path
      class="ink"
      d="M19 17 L25 24 Q32 21 39 24 L45 17 L46 37 Q46 49 32 52 Q18 49 18 37 Z"
    />
    <circle cx="26" cy="33" r="5.5" class="face-fill" />
    <circle cx="38" cy="33" r="5.5" class="face-fill" />
    <circle cx="26" cy="33" r="2.3" class="ink" />
    <circle cx="38" cy="33" r="2.3" class="ink" />
    <path d="M32 35 L29.2 41.5 L34.8 41.5 Z" class="face-fill" />
  </svg>
</span>

<style>
  .coin {
    display: inline-block;
    flex: none;
    perspective: 240px;
    cursor: default;
    touch-action: manipulation;
  }

  svg {
    display: block;
    backface-visibility: visible;
  }

  .face {
    fill: var(--surface);
    stroke: var(--text-muted);
    stroke-width: 3;
  }

  .beads {
    fill: none;
    stroke: var(--text-muted);
    stroke-width: 1.5;
    stroke-dasharray: 1.2 3.2;
    stroke-linecap: round;
  }

  .ink {
    fill: var(--text-muted);
  }

  .face-fill {
    fill: var(--surface);
  }

  .turning svg {
    animation: turn 0.7s ease-in-out;
  }

  .spinning svg {
    animation: turn 0.9s linear infinite;
  }

  @keyframes turn {
    to {
      transform: rotateY(360deg);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .turning svg,
    .spinning svg {
      animation: none;
    }
  }
</style>

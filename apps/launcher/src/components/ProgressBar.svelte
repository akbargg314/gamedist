<script lang="ts">
  import { formatBytes, percent, stepLabel } from '../lib/format';
  import type { Operation } from '../lib/state.svelte';

  let { op, compact = false }: { op: Operation; compact?: boolean } = $props();

  const pct = $derived(percent(op.done, op.total));
  const label = $derived(stepLabel(op));
  const indeterminate = $derived(op.total === 0);
</script>

<div class="progress" class:compact>
  <div class="meta">
    <span class="phase">{label}</span>
    {#if !indeterminate}
      <span class="nums">{formatBytes(op.done)} / {formatBytes(op.total)} · {pct.toFixed(0)}%</span>
    {/if}
  </div>
  <div
    class="track"
    role="progressbar"
    aria-label="{op.title}: {label}"
    aria-valuemin="0"
    aria-valuemax="100"
    aria-valuenow={indeterminate ? undefined : Math.round(pct)}
  >
    <div class="fill" class:indeterminate style:width={indeterminate ? '30%' : `${pct}%`}></div>
  </div>
</div>

<style>
  .progress {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 12px;
    font-size: 0.85rem;
  }
  .compact .meta {
    font-size: 0.75rem;
  }
  .phase {
    font-weight: 600;
  }
  .nums {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .track {
    height: 8px;
    border-radius: 99px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .compact .track {
    height: 4px;
  }
  .fill {
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, var(--accent), var(--accent-2));
    transition: width 0.15s linear;
  }
  .fill.indeterminate {
    animation: slide 1.2s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .fill {
      transition: none;
    }
    .fill.indeterminate {
      animation: none;
      width: 100% !important;
      opacity: 0.4;
    }
  }
</style>

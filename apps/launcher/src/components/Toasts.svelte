<script lang="ts">
  import { app } from '../lib/state.svelte';
</script>

<div class="toasts" role="status" aria-live="polite">
  {#each app.toasts as t (t.id)}
    <div class="toast {t.kind}">
      <span class="text">{t.text}</span>
      <button type="button" class="icon-btn" aria-label="Dismiss" onclick={() => app.dismiss(t.id)}>✕</button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 200;
    display: grid;
    gap: 8px;
    width: min(420px, calc(100vw - 32px));
  }
  .toast {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border-radius: var(--radius);
    background: var(--surface-2);
    border: 1px solid var(--border);
    border-left: 4px solid var(--accent);
    box-shadow: var(--shadow-lg);
  }
  .toast.error {
    border-left-color: var(--danger);
  }
  .toast.success {
    border-left-color: var(--success);
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    font-size: 0.9rem;
  }
</style>

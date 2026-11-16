<script lang="ts" module>
  // Stack of open dialogs: only the topmost one traps focus.
  const stack: symbol[] = [];
</script>

<script lang="ts">
  import { onMount, tick, type Snippet } from 'svelte';

  interface Props {
    title: string;
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
    /** CSS selector inside the dialog to focus first (default: first focusable). */
    initialFocus?: string;
    variant?: 'normal' | 'danger';
    wide?: boolean;
  }

  let { title, onclose, children, footer, initialFocus, variant = 'normal', wide = false }: Props = $props();

  let panel: HTMLDivElement;
  const id = `dlg-${Math.random().toString(36).slice(2)}`;
  const FOCUSABLE =
    'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

  function focusables(): HTMLElement[] {
    return Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((el) => el.offsetParent !== null);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      onclose();
      return;
    }
    if (e.key !== 'Tab') return;
    const els = focusables();
    if (!els.length) {
      e.preventDefault();
      panel.focus();
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || !panel.contains(active))) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (active === last || !panel.contains(active))) {
      e.preventDefault();
      first.focus();
    }
  }

  onMount(() => {
    const previous = document.activeElement as HTMLElement | null;
    const me = Symbol(id);
    stack.push(me);
    void tick().then(() => {
      const target = (initialFocus && panel.querySelector<HTMLElement>(initialFocus)) || focusables()[0] || panel;
      target.focus();
    });
    // Keep focus inside while open.
    const onFocusIn = (e: FocusEvent) => {
      if (stack[stack.length - 1] !== me) return;
      if (panel && !panel.contains(e.target as Node)) (focusables()[0] ?? panel).focus();
    };
    document.addEventListener('focusin', onFocusIn);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      stack.splice(stack.indexOf(me), 1);
      previous?.focus?.();
    };
  });
</script>

<div class="backdrop" role="presentation">
  <div
    bind:this={panel}
    class="panel"
    class:danger={variant === 'danger'}
    class:wide
    role={variant === 'danger' ? 'alertdialog' : 'dialog'}
    aria-modal="true"
    aria-labelledby={id}
    tabindex="-1"
    {onkeydown}
  >
    <header>
      <h2 {id}>{title}</h2>
      <button class="icon-btn" type="button" aria-label="Close" onclick={onclose}>✕</button>
    </header>
    <div class="body">
      {@render children()}
    </div>
    {#if footer}
      <footer>{@render footer()}</footer>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: 16px;
    background: var(--backdrop);
    backdrop-filter: blur(3px);
  }
  .panel {
    width: min(560px, 100%);
    max-height: calc(100vh - 32px);
    display: flex;
    flex-direction: column;
    background: var(--surface-2);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    outline: none;
  }
  .panel.wide {
    width: min(820px, 100%);
  }
  .panel.danger {
    border: 2px solid var(--danger);
  }
  header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px 8px;
  }
  .danger header h2 {
    color: var(--danger);
  }
  h2 {
    flex: 1;
    margin: 0;
    font-size: 1.15rem;
  }
  .body {
    padding: 8px 20px 16px;
    overflow: auto;
    overflow-wrap: anywhere;
  }
  footer {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 20px 16px;
    border-top: 1px solid var(--border);
  }
</style>

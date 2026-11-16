<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { openExternal } from '../lib/api';

  let { source }: { source: string } = $props();

  // renderMarkdown escapes all input and emits only allow-listed tags.
  const html = $derived(renderMarkdown(source));

  function onclick(e: MouseEvent) {
    const a = (e.target as HTMLElement | null)?.closest?.('a[data-external]');
    if (!a) return;
    e.preventDefault();
    void openExternal(a.getAttribute('href') ?? '');
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="md" {onclick}>{@html html}</div>

<style>
  .md {
    line-height: 1.6;
    overflow-wrap: anywhere;
    color: var(--text);
  }
  .md :global(h3),
  .md :global(h4),
  .md :global(h5),
  .md :global(h6) {
    margin: 1.1em 0 0.4em;
    line-height: 1.3;
  }
  .md :global(h3) {
    font-size: 1.15rem;
  }
  .md :global(h4) {
    font-size: 1.02rem;
  }
  .md :global(p),
  .md :global(ul),
  .md :global(ol) {
    margin: 0.5em 0;
  }
  .md :global(ul),
  .md :global(ol) {
    padding-left: 1.4em;
  }
  .md :global(code) {
    font-family: var(--mono);
    font-size: 0.9em;
    background: var(--surface-3);
    padding: 0.1em 0.35em;
    border-radius: 4px;
  }
  .md :global(pre) {
    background: var(--surface-3);
    padding: 10px 12px;
    border-radius: var(--radius);
    overflow-x: auto;
    max-width: 100%;
  }
  .md :global(pre code) {
    background: none;
    padding: 0;
  }
  .md :global(blockquote) {
    margin: 0.6em 0;
    padding: 0.1em 1em;
    border-left: 3px solid var(--accent);
    color: var(--text-dim);
  }
  .md :global(a) {
    color: var(--accent);
  }
  .md :global(hr) {
    border: 0;
    border-top: 1px solid var(--border);
  }
  .md > :global(:first-child) {
    margin-top: 0;
  }
</style>

<script lang="ts">
  import Modal from './Modal.svelte';
  import type { KeyChangePrompt } from '../lib/state.svelte';

  let { prompt }: { prompt: KeyChangePrompt } = $props();

  let understood = $state(false);
  const cancel = () => prompt.resolve(false);
</script>

<Modal title="Developer key changed" onclose={cancel} variant="danger" initialFocus="[data-cancel]">
  <div class="warn">
    <p class="lead">
      <strong>{prompt.title}</strong> is now signed by a <strong>different developer key</strong> than the one you
      trusted when you first added it.
    </p>
    <p>
      This can be a legitimate key change by the developer — or someone (including the server) trying to ship you a
      modified game. Only continue if the developer has told you about the new key through a channel you trust.
    </p>
    {#if prompt.sourceUrl}
      <p class="src">Presented by <span class="mono">{prompt.sourceUrl}</span></p>
    {/if}
    <dl class="keys">
      <dt>Pinned key (trusted)</dt>
      <dd class="mono key">{prompt.pinned ?? 'unknown'}</dd>
      <dt>Presented key (new)</dt>
      <dd class="mono key new">{prompt.presented ?? 'unknown'}</dd>
    </dl>
    <label class="check">
      <input type="checkbox" bind:checked={understood} />
      <span>I have verified the new key with the developer.</span>
    </label>
  </div>

  {#snippet footer()}
    <button type="button" class="btn primary" data-cancel onclick={cancel}>Cancel</button>
    <button type="button" class="btn danger" disabled={!understood} onclick={() => prompt.resolve(true)}>
      I trust the new key
    </button>
  {/snippet}
</Modal>

<style>
  .lead {
    font-size: 1.02rem;
  }
  .src {
    color: var(--text-dim);
    font-size: 0.9rem;
  }
  .keys {
    display: grid;
    gap: 4px;
    margin: 12px 0;
  }
  dt {
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    margin-top: 8px;
  }
  dd {
    margin: 0;
  }
  .key {
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--surface-3);
    word-break: break-all;
    font-size: 0.85rem;
  }
  .key.new {
    outline: 1px solid var(--danger);
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
</style>

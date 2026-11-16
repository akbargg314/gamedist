<script lang="ts">
  import Modal from './Modal.svelte';
  import GameImage from './GameImage.svelte';
  import InstallForm from './InstallForm.svelte';
  import { app } from '../lib/state.svelte';
  import { assetUrl, errorMessage, looksLikeGameUrl } from '../lib/api';
  import { hostOf } from '../lib/format';
  import type { LibraryEntry } from '../lib/types';
  import { untrack } from 'svelte';

  let { initialUrl = '' }: { initialUrl?: string } = $props();

  let url = $state(untrack(() => initialUrl));
  let busy = $state(false);
  let error = $state<string | null>(null);
  let added = $state<LibraryEntry | null>(null);
  let dir = $state('');
  let channel = $state('');

  const valid = $derived(looksLikeGameUrl(url));

  async function fetchGame(e?: SubmitEvent) {
    e?.preventDefault();
    if (!valid || busy) return;
    busy = true;
    error = null;
    try {
      added = await app.addGame(url.trim());
      if (!added) error = 'Cancelled: the game was not added because its developer key changed.';
    } catch (err) {
      error = errorMessage(err);
    } finally {
      busy = false;
    }
  }

  function openPage() {
    if (added) app.select(added.game_id);
    app.closeAddDialog();
  }

  function installNow() {
    if (!added) return;
    const id = added.game_id;
    openPage();
    void app.install(id, { dir: dir || null, channel: channel || null });
  }

  // Auto-fetch when opened from a deep link.
  if (untrack(() => initialUrl) && looksLikeGameUrl(untrack(() => initialUrl))) void fetchGame();
</script>

<Modal title={added ? 'Game added' : 'Add a game'} onclose={() => app.closeAddDialog()} initialFocus={added ? undefined : '#add-url'}>
  {#if !added}
    <form id="add-form" onsubmit={fetchGame}>
      <label class="field" for="add-url">
        <span>Game URL</span>
        <input
          id="add-url"
          class="input mono"
          type="url"
          placeholder="https://host.example/games/cool-game"
          bind:value={url}
          spellcheck="false"
          autocomplete="off"
          aria-invalid={url !== '' && !valid}
          aria-describedby="add-help"
        />
      </label>
      <p id="add-help" class="help">
        Paste a game page link (<span class="mono">https://host/games/&lt;slug&gt;</span>) or a
        <span class="mono">gamedist://</span> link. The developer's signing key is pinned on first add.
      </p>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
    </form>
  {:else}
    <div class="preview">
      <div class="icon"><GameImage src={assetUrl(added.asset_base, added.icon)} title={added.title} /></div>
      <div class="info">
        <h3>{added.title}</h3>
        {#if added.summary}<p>{added.summary}</p>{/if}
        <p class="meta">from {added.sources.map(hostOf).join(', ')}</p>
        <p class="meta">Developer key <span class="mono key">{added.pinned_key}</span></p>
      </div>
    </div>
    {#if added.installed}
      <p class="help">Already installed ({added.installed.version}).</p>
    {:else}
      <InstallForm game={added} bind:dir bind:channel />
    {/if}
  {/if}

  {#snippet footer()}
    {#if !added}
      <button type="button" class="btn" onclick={() => app.closeAddDialog()}>Cancel</button>
      <button type="submit" form="add-form" class="btn primary" disabled={!valid || busy}>
        {busy ? 'Fetching…' : 'Add game'}
      </button>
    {:else}
      <button type="button" class="btn" onclick={openPage}>Open game page</button>
      {#if !added.installed}
        <button type="button" class="btn primary" onclick={installNow} disabled={!dir}>Install</button>
      {/if}
    {/if}
  {/snippet}
</Modal>

<style>
  .help {
    color: var(--text-dim);
    font-size: 0.88rem;
  }
  .error {
    color: var(--danger);
  }
  .preview {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    margin-bottom: 16px;
  }
  .icon {
    width: 72px;
    height: 72px;
    flex: none;
    font-size: 2rem;
  }
  .info {
    min-width: 0;
  }
  h3 {
    margin: 0 0 4px;
  }
  .info p {
    margin: 2px 0;
  }
  .meta {
    color: var(--text-dim);
    font-size: 0.85rem;
  }
  .key {
    word-break: break-all;
  }
</style>

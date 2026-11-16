<script lang="ts">
  import GameImage from './GameImage.svelte';
  import { app } from '../lib/state.svelte';
  import * as api from '../lib/api';
  import type { DiscoverEntry } from '../lib/types';

  let query = $state('');
  let loading = $state(false);
  let results = $state<DiscoverEntry[]>([]);
  let errors = $state<string[]>([]);
  let adding = $state<Record<string, boolean>>({});
  let catalogCount = $state<number | null>(null);
  let seq = 0;

  async function search() {
    const mine = ++seq;
    loading = true;
    try {
      const [r, cats] = await Promise.all([api.discover(query), api.listCatalogs()]);
      if (mine !== seq) return;
      results = r.games;
      errors = r.errors;
      catalogCount = cats.length;
    } catch (e) {
      if (mine === seq) errors = [api.errorMessage(e)];
    } finally {
      if (mine === seq) loading = false;
    }
  }

  // Debounced search as the user types.
  let timer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    void query;
    clearTimeout(timer);
    timer = setTimeout(search, 300);
    return () => clearTimeout(timer);
  });

  async function add(g: DiscoverEntry) {
    adding = { ...adding, [g.url]: true };
    try {
      const entry = await app.addGame(g.url);
      if (entry) {
        results = results.map((r) => (r.game_id === g.game_id ? { ...r, in_library: true } : r));
        app.toast('success', `${entry.title} added to your library`);
      }
    } catch (e) {
      app.toast('error', `Could not add ${g.title}: ${api.errorMessage(e)}`);
    } finally {
      adding = { ...adding, [g.url]: false };
    }
  }

  function open(g: DiscoverEntry) {
    if (app.byId(g.game_id)) app.select(g.game_id);
  }
</script>

<section class="page" aria-labelledby="disc-title">
  <header class="head">
    <div>
      <h1 id="disc-title">Discover</h1>
      <p class="sub">Games from the catalog servers you follow. Manage them in Settings.</p>
    </div>
    <form
      class="search"
      role="search"
      onsubmit={(e) => {
        e.preventDefault();
        void search();
      }}
    >
      <label for="disc-q" class="sr-only">Search catalogs</label>
      <input id="disc-q" class="input" type="search" placeholder="Search games" bind:value={query} />
    </form>
  </header>

  {#if errors.length}
    <div class="errors" role="status">
      {#each errors as e, i (i)}<p class="mono">{e}</p>{/each}
    </div>
  {/if}

  {#if catalogCount === 0}
    <div class="empty">
      <p>You are not following any catalog servers yet.</p>
      <button type="button" class="btn primary" onclick={() => (app.view = 'settings')}>Add a catalog server</button>
    </div>
  {:else if !loading && results.length === 0}
    <p class="dim">{query ? `No games match “${query}”.` : 'No games found.'}</p>
  {/if}

  <ul class="grid" aria-busy={loading}>
    {#each results as g (g.server + g.game_id)}
      <li class="card">
        <div class="art">
          <GameImage src={api.assetUrl(api.discoverAssetBase(g), g.banner ?? g.icon)} title={g.title} kind="card" />
        </div>
        <div class="body">
          <h2 class="title">{g.title}</h2>
          <p class="summary">{g.summary}</p>
          <p class="meta">
            <span class="server">{g.server_name || g.server}</span>
            {#if g.channels?.stable}<span>· {g.channels.stable}</span>{/if}
          </p>
          <div class="actions">
            {#if g.in_library}
              <button type="button" class="btn" onclick={() => open(g)}>In library ›</button>
            {:else}
              <button type="button" class="btn primary" onclick={() => add(g)} disabled={adding[g.url]}>
                {adding[g.url] ? 'Adding…' : 'Add to library'}
              </button>
            {/if}
          </div>
        </div>
      </li>
    {/each}
  </ul>
  {#if loading}<p class="dim" role="status">Searching…</p>{/if}
</section>

<style>
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 20px;
  }
  h1 {
    margin: 0;
  }
  .sub,
  .dim {
    color: var(--text-dim);
  }
  .sub {
    margin: 4px 0 0;
  }
  .search {
    width: min(320px, 100%);
  }
  .search input {
    width: 100%;
  }
  .errors {
    margin-bottom: 16px;
    padding: 8px 12px;
    border: 1px solid var(--warning);
    border-radius: var(--radius);
    background: var(--warning-soft);
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }
  .errors p {
    margin: 4px 0;
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
    gap: 16px;
  }
  .card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface-1);
    overflow: hidden;
    min-width: 0;
  }
  .art {
    aspect-ratio: 16 / 9;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px;
    flex: 1;
  }
  .title {
    margin: 0;
    font-size: 1rem;
    overflow-wrap: anywhere;
  }
  .summary {
    margin: 0;
    color: var(--text-dim);
    font-size: 0.88rem;
    flex: 1;
  }
  .meta {
    margin: 0;
    font-size: 0.78rem;
    color: var(--text-dim);
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .server {
    overflow-wrap: anywhere;
  }
  .actions {
    margin-top: 6px;
  }
  .empty {
    padding: 32px;
    text-align: center;
    border: 1px dashed var(--border);
    border-radius: var(--radius-lg);
  }
</style>

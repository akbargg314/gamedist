<script lang="ts">
  import GameImage from './GameImage.svelte';
  import { app, statusOf, type GameStatus } from '../lib/state.svelte';
  import { assetUrl } from '../lib/api';
  import { percent } from '../lib/format';

  let query = $state('');

  const filtered = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return app.games;
    return app.games.filter((g) => `${g.title} ${g.slug}`.toLowerCase().includes(q));
  });

  const STATUS_LABEL: Record<GameStatus, string> = {
    'not-installed': 'Not installed',
    busy: 'Working…',
    running: 'Running',
    update: 'Update available',
    installed: 'Installed',
  };
</script>

<aside class="sidebar" aria-label="Library">
  <div class="top">
    <button type="button" class="btn primary add" onclick={() => app.openAddDialog()}>+ Add game</button>
    <label class="search">
      <span class="sr-only">Search library</span>
      <input class="input" type="search" placeholder="Search library" bind:value={query} />
    </label>
  </div>

  <nav class="list" aria-label="Games">
    <button
      type="button"
      class="item home"
      class:active={app.view === 'library' && !app.selectedId}
      aria-current={app.view === 'library' && !app.selectedId ? 'page' : undefined}
      onclick={() => app.select(null)}
    >
      <span class="all" aria-hidden="true">▦</span>
      <span class="name">All games <span class="count">({app.games.length})</span></span>
    </button>
    {#each filtered as g (g.game_id)}
      {@const status = statusOf(g)}
      {@const op = app.ops[g.game_id]}
      <button
        type="button"
        class="item"
        class:active={app.view === 'library' && app.selectedId === g.game_id}
        class:dim={status === 'not-installed'}
        aria-current={app.view === 'library' && app.selectedId === g.game_id ? 'page' : undefined}
        title="{g.title} — {STATUS_LABEL[status]}"
        onclick={() => app.select(g.game_id)}
      >
        <span class="icon"><GameImage src={assetUrl(g.asset_base, g.icon)} title={g.title} /></span>
        <span class="name">{g.title}</span>
        <span class="sr-only">, {STATUS_LABEL[status]}</span>
        {#if status === 'update'}<span class="dot update" aria-hidden="true"></span>{/if}
        {#if status === 'running'}<span class="dot running" aria-hidden="true"></span>{/if}
        {#if op}
          <span class="mini" aria-hidden="true"><span style:width="{percent(op.done, op.total)}%"></span></span>
        {/if}
      </button>
    {:else}
      <p class="empty">
        {app.games.length ? 'No matches.' : app.loaded ? 'Your library is empty.' : 'Loading…'}
      </p>
    {/each}
  </nav>
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--surface-1);
    border-right: 1px solid var(--border);
  }
  .top {
    display: grid;
    gap: 8px;
    padding: 12px;
  }
  .add {
    width: 100%;
  }
  .list {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 0 6px 12px;
  }
  .item {
    position: relative;
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    border-radius: var(--radius);
    background: none;
    color: var(--text);
    text-align: left;
    font: inherit;
    cursor: pointer;
  }
  .item:hover {
    background: var(--surface-2);
  }
  .item.active {
    background: var(--accent-soft);
    color: var(--text-strong);
  }
  .item.dim .name {
    color: var(--text-dim);
  }
  .icon,
  .all {
    width: 28px;
    height: 28px;
    flex: none;
    font-size: 0.9rem;
  }
  .all {
    display: grid;
    place-items: center;
    color: var(--text-dim);
    font-size: 1.1rem;
  }
  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    color: var(--text-dim);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .dot.update {
    background: var(--accent-2);
  }
  .dot.running {
    background: var(--success);
  }
  .mini {
    position: absolute;
    left: 46px;
    right: 8px;
    bottom: 2px;
    height: 2px;
    background: var(--surface-3);
    border-radius: 2px;
    overflow: hidden;
  }
  .mini span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .empty {
    padding: 12px;
    color: var(--text-dim);
    font-size: 0.9rem;
  }
</style>

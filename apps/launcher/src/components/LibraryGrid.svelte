<script lang="ts">
  import GameImage from './GameImage.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import { app, statusOf } from '../lib/state.svelte';
  import { assetUrl } from '../lib/api';
  import { formatBytes, installedSize } from '../lib/format';

  const installedCount = $derived(app.games.filter((g) => g.installed).length);
  const installedBytes = $derived(app.games.reduce((s, g) => s + (installedSize(g) ?? 0), 0));
</script>

<section class="page" aria-labelledby="lib-title">
  <header class="head">
    <div>
      <h1 id="lib-title">Library</h1>
      <p class="sub">
        {app.games.length} games · {installedCount} installed{installedBytes ? ` · ${formatBytes(installedBytes)} on disk` : ''}
      </p>
    </div>
    <button type="button" class="btn primary" onclick={() => app.openAddDialog()}>+ Add game</button>
  </header>

  {#if app.loaded && app.games.length === 0}
    <div class="empty">
      <h2>No games yet</h2>
      <p>Paste a game link from any GameDist server, or browse the catalogs you follow.</p>
      <div class="actions">
        <button type="button" class="btn primary" onclick={() => app.openAddDialog()}>Add a game by URL</button>
        <button type="button" class="btn" onclick={() => (app.view = 'discover')}>Open Discover</button>
      </div>
    </div>
  {:else}
    <ul class="grid">
      {#each app.games as g (g.game_id)}
        {@const status = statusOf(g)}
        {@const op = app.ops[g.game_id]}
        <li>
          <button type="button" class="card" onclick={() => app.select(g.game_id)} aria-label="{g.title}, {status.replace('-', ' ')}">
            <span class="art">
              <GameImage src={assetUrl(g.asset_base, g.banner ?? g.icon)} title={g.title} kind="card" />
              {#if status === 'update'}<span class="tag update">Update</span>{/if}
              {#if status === 'running'}<span class="tag running">Running</span>{/if}
              {#if status === 'not-installed'}<span class="tag muted">Not installed</span>{/if}
            </span>
            <span class="body">
              <span class="title">{g.title}</span>
              {#if op}
                <ProgressBar {op} compact />
              {:else}
                {@const size = installedSize(g)}
                {#if size}<span class="size">{formatBytes(size)} on disk</span>{/if}
                <span class="summary">{g.summary || ' '}</span>
              {/if}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
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
  .sub {
    margin: 4px 0 0;
    color: var(--text-dim);
  }
  .grid {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: 16px;
  }
  .card {
    display: flex;
    flex-direction: column;
    width: 100%;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--surface-1);
    color: var(--text);
    text-align: left;
    font: inherit;
    cursor: pointer;
    overflow: hidden;
    transition:
      transform 0.12s,
      border-color 0.12s;
  }
  .card:hover {
    transform: translateY(-2px);
    border-color: var(--accent);
  }
  .art {
    position: relative;
    aspect-ratio: 16 / 9;
    display: block;
  }
  .tag {
    position: absolute;
    top: 8px;
    right: 8px;
    padding: 2px 8px;
    border-radius: 99px;
    font-size: 0.72rem;
    font-weight: 700;
    background: var(--surface-1);
  }
  .tag.update {
    background: var(--accent-2);
    color: #111;
  }
  .tag.running {
    background: var(--success);
    color: #062;
  }
  .tag.muted {
    color: var(--text-dim);
  }
  .body {
    display: grid;
    gap: 6px;
    padding: 10px 12px 12px;
    min-width: 0;
  }
  .title {
    font-weight: 700;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .summary {
    color: var(--text-dim);
    font-size: 0.85rem;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .size {
    margin-top: -4px;
    color: var(--text-dim);
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
  }
  .empty {
    padding: 40px 24px;
    text-align: center;
    border: 1px dashed var(--border);
    border-radius: var(--radius-lg);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 8px;
  }
  @media (prefers-reduced-motion: reduce) {
    .card {
      transition: none;
    }
    .card:hover {
      transform: none;
    }
  }
</style>

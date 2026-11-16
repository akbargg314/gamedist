<script lang="ts">
  import { untrack } from 'svelte';
  import GameImage from './GameImage.svelte';
  import Markdown from './Markdown.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import Modal from './Modal.svelte';
  import InstallForm from './InstallForm.svelte';
  import { app, statusOf } from '../lib/state.svelte';
  import { assetUrl, errorMessage, looksLikeGameUrl } from '../lib/api';
  import { formatBytes, formatDate, hostOf, installedSize, seedRatio } from '../lib/format';
  import type { LibraryEntry } from '../lib/types';

  let { game }: { game: LibraryEntry } = $props();

  const status = $derived(statusOf(game));
  const op = $derived(app.ops[game.game_id]);
  const update = $derived(app.updates[game.game_id]);
  const sizeOnDisk = $derived(installedSize(game));
  let checking = $state(false);

  // Update check whenever a game page is opened (PLAN §5 flow 4).
  $effect(() => {
    const id = game.game_id;
    untrack(() => {
      checking = true;
      void app.checkUpdate(id, true).finally(() => (checking = false));
    });
  });

  // Seeding stats, polled only while an installed game's page is open.
  const installed = $derived(!!game.installed);
  $effect(() => {
    if (installed) return app.watchSeeding();
  });
  const seed = $derived(app.seedStatus(game.game_id));
  let seedBusy = $state(false);
  async function toggleSeed() {
    seedBusy = true;
    await (seed ? app.stopSeeding(game.game_id) : app.seedGame(game.game_id));
    seedBusy = false;
  }

  let showInstall = $state(false);
  let installDir = $state('');
  let installChannel = $state('');
  let showUninstall = $state(false);
  let forget = $state(false);
  let mirrorUrl = $state('');
  let mirrorBusy = $state(false);
  let mirrorError = $state<string | null>(null);

  function openInstall() {
    installDir = '';
    installChannel = '';
    showInstall = true;
  }

  function doInstall() {
    showInstall = false;
    void app.install(game.game_id, { dir: installDir || null, channel: installChannel || null });
  }

  async function recheck() {
    checking = true;
    const r = await app.checkUpdate(game.game_id, false);
    checking = false;
    if (r && !r.update_available && game.installed) app.toast('info', `${game.title} is up to date`);
  }

  async function addMirror(e: SubmitEvent) {
    e.preventDefault();
    mirrorError = null;
    if (!looksLikeGameUrl(mirrorUrl)) {
      mirrorError = 'Enter a game URL like https://mirror.example/games/' + game.slug;
      return;
    }
    mirrorBusy = true;
    try {
      const r = await app.addGame(mirrorUrl.trim());
      if (r && r.game_id !== game.game_id) {
        mirrorError = `That URL is a different game (${r.title}); it was added to your library separately.`;
      } else if (r) {
        mirrorUrl = '';
      }
    } catch (err) {
      mirrorError = errorMessage(err);
    } finally {
      mirrorBusy = false;
    }
  }

  function doUninstall() {
    showUninstall = false;
    void app.uninstall(game.game_id, forget);
  }
</script>

<article class="game" aria-labelledby="game-title">
  <div class="banner">
    <GameImage src={assetUrl(game.asset_base, game.banner)} title={game.title} kind="banner" />
    <div class="shade"></div>
  </div>

  <header class="hero">
    <div class="icon"><GameImage src={assetUrl(game.asset_base, game.icon)} title={game.title} /></div>
    <div class="titles">
      <h1 id="game-title">{game.title}</h1>
      {#if game.summary}<p class="summary">{game.summary}</p>{/if}
    </div>
  </header>

  <section class="actionbar" aria-label="Actions">
    <div class="primary-action">
      {#if status === 'busy' && op}
        <div class="op"><ProgressBar {op} /></div>
      {:else if status === 'running'}
        <button type="button" class="btn big running" disabled>● Running</button>
      {:else if status === 'not-installed'}
        <button type="button" class="btn big primary" onclick={openInstall}>Install</button>
      {:else if status === 'update'}
        <button type="button" class="btn big accent2" onclick={() => app.install(game.game_id)}>
          Update to {update?.latest}
        </button>
        <button type="button" class="btn big" onclick={() => app.launch(game.game_id)}>Play {game.installed?.version}</button>
      {:else}
        <button type="button" class="btn big play" onclick={() => app.launch(game.game_id)}>▶ Play</button>
      {/if}
    </div>

    <dl class="stats">
      <div>
        <dt>Installed</dt>
        <dd>{game.installed?.version ?? '—'}</dd>
      </div>
      <div>
        <dt>Latest ({game.channel})</dt>
        <dd>{update?.latest ?? game.channels[game.channel] ?? (checking ? '…' : '—')}</dd>
      </div>
      {#if game.installed}
        <div>
          <dt>Size on disk</dt>
          <dd title={sizeOnDisk == null ? 'Unknown: the manifest of the installed version could not be read' : undefined}>
            {sizeOnDisk == null ? '—' : formatBytes(sizeOnDisk)}
          </dd>
        </div>
      {/if}
      {#if !game.installed || (update && update.latest !== game.installed.version)}
        <div>
          <dt>{game.installed ? `Size of ${update?.latest}` : 'Size'}</dt>
          <dd>{update ? formatBytes(update.total_bytes) : '—'}</dd>
        </div>
      {/if}
      {#if update && (update.update_available || !game.installed)}
        <div>
          <dt>{game.installed ? 'Update download' : 'Download'}</dt>
          <dd>{formatBytes(update.download_bytes)}</dd>
        </div>
      {/if}
    </dl>
  </section>

  <div class="columns">
    <div class="main">
      {#if update?.changelog_md && (update.update_available || !game.installed || update.latest === game.installed?.version)}
        <section class="card">
          <h2>What's new in {update.latest}</h2>
          <Markdown source={update.changelog_md} />
        </section>
      {/if}
      <section class="card">
        <h2>About</h2>
        {#if game.description_md}
          <Markdown source={game.description_md} />
        {:else}
          <p class="dim">The developer has not written a description.</p>
        {/if}
      </section>
      {#if game.screenshots.length}
        <section class="card">
          <h2>Screenshots</h2>
          <div class="shots">
            {#each game.screenshots as s, i (s)}
              <div class="shot"><GameImage src={assetUrl(game.asset_base, s)} title={game.title} kind="card" alt="Screenshot {i + 1}" /></div>
            {/each}
          </div>
        </section>
      {/if}
    </div>

    <aside class="side">
      <section class="card">
        <h2>Manage</h2>
        {#if game.installed}
          <dl class="kv">
            <dt>Folder</dt>
            <dd class="mono wrap">{game.installed.dir}</dd>
            <dt>Platform</dt>
            <dd>{game.installed.platform}</dd>
            <dt>Installed on</dt>
            <dd>{formatDate(game.installed.installed_at)}</dd>
            <dt>Size</dt>
            <dd>{sizeOnDisk == null ? 'Unknown' : formatBytes(sizeOnDisk)}</dd>
          </dl>
          <p class="seeding small" class:on={!!seed}>
            {#if seed}
              Seeding · uploaded {formatBytes(seed.uploaded_bytes)} (ratio {seedRatio(seed)})
            {:else if app.settings?.seed}
              Not seeding
            {:else}
              Not seeding (seeding is off in Settings)
            {/if}
          </p>
        {/if}
        <div class="stack">
          <button type="button" class="btn" onclick={recheck} disabled={checking || !!op}>
            {checking ? 'Checking…' : 'Check for updates'}
          </button>
          {#if game.installed}
            <button type="button" class="btn" onclick={() => app.verify(game.game_id)} disabled={!!op || status === 'running'}>
              Verify files
            </button>
            {#if seed || app.settings?.seed}
              <button type="button" class="btn" onclick={toggleSeed} disabled={seedBusy || !!op}>
                {seedBusy ? (seed ? 'Stopping…' : 'Checking files…') : seed ? 'Stop seeding' : 'Seed this game'}
              </button>
            {/if}
          {/if}
          <button
            type="button"
            class="btn danger-outline"
            onclick={() => {
              forget = !game.installed;
              showUninstall = true;
            }}
            disabled={!!op || status === 'running'}
          >
            {game.installed ? 'Uninstall…' : 'Remove from library…'}
          </button>
        </div>
      </section>

      <section class="card">
        <h2>Sources</h2>
        <ul class="sources">
          {#each game.sources as s (s)}
            <li><span class="mono wrap">{s}</span></li>
          {/each}
        </ul>
        <form class="mirror" onsubmit={addMirror}>
          <label class="field" for="mirror-url">
            <span>Add a mirror</span>
            <input
              id="mirror-url"
              class="input mono"
              type="url"
              placeholder="https://mirror.example/games/{game.slug}"
              bind:value={mirrorUrl}
              spellcheck="false"
            />
          </label>
          <button type="submit" class="btn" disabled={mirrorBusy || !mirrorUrl}>{mirrorBusy ? 'Adding…' : 'Add'}</button>
        </form>
        {#if mirrorError}<p class="error" role="alert">{mirrorError}</p>{/if}
      </section>

      <section class="card">
        <h2>Developer key</h2>
        <p class="dim small">
          Pinned when you first added this game. Every release must be signed by this key.
        </p>
        <p class="mono wrap key">{game.pinned_key}</p>
        <dl class="kv">
          <dt>Game ID</dt>
          <dd class="mono wrap">{game.game_id}</dd>
          <dt>Slug</dt>
          <dd class="mono">{game.slug}</dd>
        </dl>
      </section>
    </aside>
  </div>
</article>

{#if showInstall}
  <Modal title="Install {game.title}" onclose={() => (showInstall = false)}>
    <InstallForm {game} bind:dir={installDir} bind:channel={installChannel} />
    {#if update}
      <p class="dim small">Download size: {formatBytes(update.download_bytes)} · from {game.sources.map(hostOf).join(', ')}</p>
    {/if}
    {#snippet footer()}
      <button type="button" class="btn" onclick={() => (showInstall = false)}>Cancel</button>
      <button type="button" class="btn primary" onclick={doInstall} disabled={!installDir}>Install</button>
    {/snippet}
  </Modal>
{/if}

{#if showUninstall}
  <Modal title={game.installed ? `Uninstall ${game.title}?` : `Remove ${game.title}?`} onclose={() => (showUninstall = false)} initialFocus="[data-cancel]">
    {#if game.installed}
      <p>This deletes the game files in <span class="mono wrap">{game.installed.dir}</span>. Save games stored there may be lost.</p>
      <label class="check">
        <input type="checkbox" bind:checked={forget} />
        <span>Also remove from library (forgets the pinned developer key)</span>
      </label>
    {:else}
      <p>The game and its pinned developer key will be removed from your library.</p>
    {/if}
    {#snippet footer()}
      <button type="button" class="btn" data-cancel onclick={() => (showUninstall = false)}>Cancel</button>
      <button type="button" class="btn danger" onclick={doUninstall}>{game.installed ? 'Uninstall' : 'Remove'}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .game {
    position: relative;
    margin: calc(-1 * var(--pad)) calc(-1 * var(--pad)) 0;
  }
  .banner {
    position: relative;
    height: clamp(180px, 28vw, 320px);
  }
  .shade {
    position: absolute;
    inset: 0;
    background: linear-gradient(to bottom, transparent 30%, var(--bg) 100%);
  }
  .hero {
    position: relative;
    display: flex;
    align-items: flex-end;
    gap: 18px;
    margin-top: -64px;
    padding: 0 var(--pad);
  }
  .icon {
    width: 96px;
    height: 96px;
    flex: none;
    font-size: 2.6rem;
    box-shadow: var(--shadow-lg);
    border-radius: var(--radius);
  }
  .titles {
    min-width: 0;
    padding-bottom: 4px;
  }
  h1 {
    margin: 0;
    font-size: clamp(1.6rem, 3vw, 2.2rem);
    overflow-wrap: anywhere;
    color: var(--text-strong);
  }
  .summary {
    margin: 4px 0 0;
    color: var(--text-dim);
  }
  .actionbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 16px 32px;
    margin: 20px var(--pad);
    padding: 14px 16px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
  }
  .primary-action {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    flex: 0 1 auto;
    min-width: 220px;
  }
  .op {
    width: min(360px, 100%);
    min-width: 220px;
  }
  .stats {
    display: flex;
    flex-wrap: wrap;
    gap: 8px 28px;
    margin: 0;
  }
  .stats dt {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
  }
  .stats dd {
    margin: 2px 0 0;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .columns {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(240px, 320px);
    gap: 20px;
    padding: 0 var(--pad) var(--pad);
  }
  @media (max-width: 1050px) {
    .columns {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .main,
  .side {
    display: grid;
    gap: 20px;
    align-content: start;
    min-width: 0;
  }
  .card {
    padding: 16px 18px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 0;
  }
  .card h2 {
    margin: 0 0 10px;
    font-size: 1rem;
  }
  .shots {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 10px;
  }
  .shot {
    aspect-ratio: 16 / 9;
    border-radius: var(--radius);
    overflow: hidden;
  }
  .stack {
    display: grid;
    gap: 8px;
  }
  .kv {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 4px 12px;
    margin: 0 0 12px;
    font-size: 0.88rem;
  }
  .kv dt {
    color: var(--text-dim);
  }
  .kv dd {
    margin: 0;
    min-width: 0;
  }
  .wrap {
    overflow-wrap: anywhere;
    word-break: break-all;
  }
  .key {
    padding: 8px 10px;
    background: var(--surface-3);
    border-radius: var(--radius);
    font-size: 0.85rem;
  }
  .sources {
    list-style: none;
    margin: 0 0 12px;
    padding: 0;
    display: grid;
    gap: 4px;
    font-size: 0.88rem;
  }
  .mirror {
    display: flex;
    gap: 8px;
    align-items: flex-end;
  }
  .mirror .field {
    flex: 1;
    min-width: 0;
  }
  .error {
    color: var(--danger);
    font-size: 0.88rem;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 0.85rem;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .seeding {
    margin: 0 0 12px;
    color: var(--text-dim);
  }
  .seeding.on {
    color: var(--success, var(--accent));
    font-weight: 600;
  }
</style>

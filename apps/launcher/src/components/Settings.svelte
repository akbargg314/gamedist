<script lang="ts">
  import { app } from '../lib/state.svelte';
  import * as api from '../lib/api';
  import type { Settings, SettingsPatch, Transport } from '../lib/types';

  const MiB = 1024 * 1024;

  // Local editable copy; rates are edited in MiB/s (blank = unlimited).
  let form = $state<Settings | null>(null);
  let bwMiB = $state('');
  let upMiB = $state('');
  let portStr = $state('');
  let saving = $state(false);
  let saveError = $state<string | null>(null);

  function load(s: Settings) {
    form = { ...s };
    bwMiB = s.bandwidth_limit ? String(+(s.bandwidth_limit / MiB).toFixed(2)) : '';
    upMiB = s.upload_limit ? String(+(s.upload_limit / MiB).toFixed(2)) : '';
    portStr = s.listen_port ? String(s.listen_port) : '';
  }

  $effect(() => {
    if (app.settings && !form) load(app.settings);
  });

  // One-off: is the torrent engine running, and on which port?
  $effect(() => {
    void app.refreshSeeding();
  });

  function rate(v: string): number | null {
    const n = Number(v);
    return v.trim() && Number.isFinite(n) && n > 0 ? Math.round(n * MiB) : null;
  }

  const bwInvalid = $derived(bwMiB.trim() !== '' && !(Number(bwMiB) >= 0));
  const upInvalid = $derived(upMiB.trim() !== '' && !(Number(upMiB) >= 0));
  /** Blank or 0 = any free port. */
  const port = $derived(portStr.trim() === '' ? 0 : Number(portStr));
  const portInvalid = $derived(!Number.isInteger(port) || port < 0 || port > 65535);
  const concurrencyInvalid = $derived(!form || !Number.isInteger(form.concurrency) || form.concurrency < 1 || form.concurrency > 64);
  const invalid = $derived(bwInvalid || upInvalid || portInvalid || concurrencyInvalid || !form?.install_root.trim());

  /**
   * Only the fields the user changed. Saving sends just these, so settings
   * this page does not show (torrent_stall_secs) keep their stored value.
   */
  const patch = $derived.by((): SettingsPatch => {
    const cur = app.settings;
    if (!form || !cur) return {};
    const edited: Required<SettingsPatch> = {
      install_root: form.install_root.trim(),
      bandwidth_limit: rate(bwMiB),
      concurrency: form.concurrency,
      keep_chunks: form.keep_chunks,
      transport: form.transport,
      seed: form.seed,
      upload_limit: rate(upMiB),
      wine: form.wine?.trim() || null,
      listen_port: portInvalid ? cur.listen_port : port,
    };
    const out: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(edited)) if (v !== cur[k as keyof Settings]) out[k] = v;
    return out as SettingsPatch;
  });
  const dirty = $derived(Object.keys(patch).length > 0);

  const torrentInUse = $derived(!!form && (form.seed || form.transport !== 'direct'));

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!form || invalid || !dirty) return;
    saving = true;
    saveError = null;
    try {
      await app.saveSettings($state.snapshot(patch));
      load(app.settings!);
      app.toast('success', 'Settings saved');
    } catch (err) {
      saveError = api.errorMessage(err);
    } finally {
      saving = false;
    }
  }

  async function browseRoot() {
    if (!form) return;
    const p = await api.pickDirectory(form.install_root, 'Default install folder');
    if (p) form.install_root = p;
  }
  async function browseWine() {
    if (!form) return;
    const p = await api.pickFile(form.wine ?? undefined, 'Wine / Proton runner');
    if (p) form.wine = p;
  }

  const transports: { value: Transport; label: string; help: string }[] = [
    { value: 'direct', label: 'Direct (HTTP)', help: 'Download chunks from the game servers only.' },
    { value: 'torrent-direct', label: 'Torrent + direct', help: 'Use BitTorrent peers, fill gaps over HTTP. Recommended.' },
    { value: 'torrent', label: 'Torrent only', help: 'Only download from peers; may be slow for rare games.' },
  ];

  // ---- catalogs ----
  let catalogs = $state<string[]>([]);
  let newCatalog = $state('');
  let catalogBusy = $state(false);
  let catalogError = $state<string | null>(null);

  $effect(() => {
    void api.listCatalogs().then((c) => (catalogs = c)).catch(() => {});
  });

  async function addCatalog(e: SubmitEvent) {
    e.preventDefault();
    const url = newCatalog.trim();
    if (!url) return;
    catalogBusy = true;
    catalogError = null;
    try {
      const name = await api.addCatalog(url);
      catalogs = await api.listCatalogs();
      newCatalog = '';
      app.toast('success', `Following catalog “${name}”`);
    } catch (err) {
      catalogError = api.errorMessage(err);
    } finally {
      catalogBusy = false;
    }
  }

  async function removeCatalog(url: string) {
    try {
      await api.removeCatalog(url);
      catalogs = await api.listCatalogs();
    } catch (err) {
      app.toast('error', api.errorMessage(err));
    }
  }
</script>

<section class="page settings" aria-labelledby="set-title">
  <h1 id="set-title">Settings</h1>

  {#if !form}
    <p class="dim">Loading…</p>
  {:else}
    <form onsubmit={save} novalidate>
      <fieldset class="card">
        <legend>Installation</legend>
        <label class="field" for="s-root">
          <span>Default install folder</span>
          <div class="row">
            <input id="s-root" class="input mono" type="text" bind:value={form.install_root} spellcheck="false" />
            <button type="button" class="btn" onclick={browseRoot}>Browse…</button>
          </div>
          <small>Games install to <span class="mono">&lt;folder&gt;/&lt;slug&gt;</span> unless you choose otherwise.</small>
        </label>
        <label class="check">
          <input type="checkbox" bind:checked={form.keep_chunks} />
          <span>
            Keep downloaded chunks
            <small>Faster re-installs and repairs, at the cost of disk space.</small>
          </span>
        </label>
        <label class="field" for="s-wine">
          <span>Wine / Proton runner</span>
          <div class="row">
            <input
              id="s-wine"
              class="input mono"
              type="text"
              placeholder="Auto-detect"
              value={form.wine ?? ''}
              oninput={(e) => form && (form.wine = e.currentTarget.value || null)}
              spellcheck="false"
            />
            <button type="button" class="btn" onclick={browseWine}>Browse…</button>
          </div>
          <small>Used to run Windows-only builds on Linux and macOS.</small>
        </label>
      </fieldset>

      <fieldset class="card">
        <legend>Downloads</legend>
        <div class="field">
          <span id="t-label">Transport</span>
          <div class="radios" role="radiogroup" aria-labelledby="t-label">
            {#each transports as t (t.value)}
              <label class="radio" class:selected={form.transport === t.value}>
                <input type="radio" name="transport" value={t.value} bind:group={form.transport} />
                <span>
                  <strong>{t.label}</strong>
                  <small>{t.help}</small>
                </span>
              </label>
            {/each}
          </div>
        </div>
        <div class="two">
          <label class="field" for="s-bw">
            <span>Download limit (MiB/s)</span>
            <input id="s-bw" class="input" type="text" inputmode="decimal" placeholder="Unlimited" bind:value={bwMiB} aria-invalid={bwInvalid} />
          </label>
          <label class="field" for="s-conc">
            <span>Parallel downloads</span>
            <input id="s-conc" class="input" type="number" min="1" max="64" step="1" bind:value={form.concurrency} aria-invalid={concurrencyInvalid} />
            {#if concurrencyInvalid}<small class="error">Between 1 and 64.</small>{/if}
          </label>
        </div>
      </fieldset>

      <fieldset class="card">
        <legend>Seeding</legend>
        <label class="check">
          <input type="checkbox" bind:checked={form.seed} />
          <span>
            Seed installed games to other players
            <small>Shares the game files you already have over BitTorrent.</small>
          </span>
        </label>
        <div class="two">
          <label class="field" for="s-up">
            <span>Upload limit (MiB/s)</span>
            <input
              id="s-up"
              class="input"
              type="text"
              inputmode="decimal"
              placeholder="Unlimited"
              bind:value={upMiB}
              disabled={!torrentInUse}
              aria-invalid={upInvalid}
            />
            <small>Also applies while downloading over BitTorrent.</small>
          </label>
          <label class="field" for="s-port">
            <span>Listen port (TCP)</span>
            <input
              id="s-port"
              class="input"
              type="text"
              inputmode="numeric"
              placeholder="Any free port"
              bind:value={portStr}
              disabled={!torrentInUse}
              aria-invalid={portInvalid}
            />
            {#if portInvalid}
              <small class="error">A port between 1 and 65535, or blank for any.</small>
            {:else}
              <small>For incoming peer connections. Blank or 0 picks a free port.</small>
            {/if}
          </label>
        </div>
        {#if app.seeding.port != null}
          <p class="dim small">
            BitTorrent is listening on port <span class="mono">{app.seeding.port}</span>. Port and limit changes take
            effect the next time GameDist starts.
          </p>
        {/if}
      </fieldset>

      <div class="savebar">
        {#if saveError}<p class="error" role="alert">{saveError}</p>{/if}
        <button type="button" class="btn" disabled={!dirty || saving} onclick={() => app.settings && load(app.settings)}>Revert</button>
        <button type="submit" class="btn primary" disabled={!dirty || invalid || saving}>{saving ? 'Saving…' : 'Save settings'}</button>
      </div>
    </form>
  {/if}

  <section class="card" aria-labelledby="cat-title">
    <h2 id="cat-title">Catalog servers</h2>
    <p class="dim small">Servers whose public game lists appear in Discover. Adding a catalog never trusts any developer key.</p>
    {#if catalogs.length}
      <ul class="catalogs">
        {#each catalogs as c (c)}
          <li>
            <span class="mono wrap">{c}</span>
            <button type="button" class="btn small danger-outline" onclick={() => removeCatalog(c)} aria-label="Remove catalog {c}">Remove</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="dim">No catalogs yet.</p>
    {/if}
    <form class="row" onsubmit={addCatalog}>
      <label class="sr-only" for="cat-url">Catalog server URL</label>
      <input id="cat-url" class="input mono" type="url" placeholder="https://host.example" bind:value={newCatalog} spellcheck="false" />
      <button type="submit" class="btn" disabled={catalogBusy || !newCatalog.trim()}>{catalogBusy ? 'Adding…' : 'Add'}</button>
    </form>
    {#if catalogError}<p class="error" role="alert">{catalogError}</p>{/if}
  </section>
</section>

<style>
  .settings {
    max-width: 760px;
  }
  h1 {
    margin: 0 0 16px;
  }
  form {
    display: grid;
    gap: 16px;
  }
  .card {
    display: grid;
    gap: 14px;
    margin: 0 0 16px;
    padding: 16px 18px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 0;
  }
  form .card {
    margin: 0;
  }
  legend,
  h2 {
    float: left;
    width: 100%;
    margin: 0;
    padding: 0;
    font-size: 1rem;
    font-weight: 700;
  }
  legend + * {
    clear: both;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .row input {
    flex: 1;
    min-width: 0;
  }
  .two {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 14px;
  }
  .check {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .check small,
  .radio small {
    display: block;
    color: var(--text-dim);
  }
  .radios {
    display: grid;
    gap: 8px;
  }
  .radio {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
  }
  .radio.selected {
    border-color: var(--accent);
    background: var(--accent-soft);
  }
  .savebar {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    align-items: center;
    gap: 8px;
  }
  .savebar .error {
    flex: 1;
    margin: 0;
  }
  .catalogs {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }
  .catalogs li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 0;
    border-bottom: 1px solid var(--border);
  }
  .wrap {
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 0.85rem;
  }
  .error {
    color: var(--danger);
  }
</style>

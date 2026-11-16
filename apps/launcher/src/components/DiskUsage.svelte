<script lang="ts">
  // Free space of the install root and of the folder of every active
  // operation (when that lives on another filesystem), with a warning when
  // the rest of an install will not fit. Polls only while mounted.
  import * as api from '../lib/api';
  import { app, type Operation } from '../lib/state.svelte';
  import { formatBytes, installedSize, percent } from '../lib/format';
  import type { DiskSpace } from '../lib/types';

  interface Target {
    path: string;
    label: string;
    op?: Operation;
  }

  const targets = $derived.by((): Target[] => {
    const out: Target[] = [];
    const root = app.settings?.install_root;
    if (root) out.push({ path: root, label: 'Install folder' });
    for (const op of app.activeOps) if (op.dir) out.push({ path: op.dir, label: op.title, op });
    return out;
  });
  const busy = $derived(app.activeOps.length > 0);
  const key = $derived(targets.map((t) => t.path).join('\n'));

  let spaces = $state<Record<string, DiskSpace>>({});
  let errors = $state<Record<string, string>>({});

  async function refresh(paths: string[]) {
    const results = await Promise.all(
      paths.map((p) =>
        api.diskSpace(p).then(
          (s) => ({ p, s, e: null as string | null }),
          (e) => ({ p, s: null, e: api.errorMessage(e) }),
        ),
      ),
    );
    const next: Record<string, DiskSpace> = {};
    const errs: Record<string, string> = {};
    for (const r of results) {
      if (r.s) next[r.p] = r.s;
      else if (r.e) errs[r.p] = r.e;
    }
    spaces = next;
    errors = errs;
  }

  // Free space changes quickly while installing; otherwise an occasional look is enough.
  $effect(() => {
    const paths = key ? key.split('\n') : [];
    void refresh(paths);
    const timer = setInterval(
      () => {
        if (document.visibilityState === 'visible') void refresh(paths);
      },
      busy ? 4000 : 30000,
    );
    return () => clearInterval(timer);
  });

  /**
   * Bytes an operation still has to put on disk, roughly: during the
   * download the remaining chunks plus the files they become (both exist
   * until the install finishes); while writing files, what is left to write.
   * Assumes the chunk store is on the same filesystem as the game.
   */
  function stillNeeds(op: Operation): number {
    const left = Math.max(0, op.total - op.done);
    if (op.phase === 'assemble') return left;
    if (op.phase === 'download') {
      // `total` counts one source only (the torrent's, then HTTP's rest), so
      // the files to write are at least what the update check announced.
      const u = op.op === 'install' ? app.updates[op.game_id] : undefined;
      return left + Math.max(op.total, u?.download_bytes ?? 0);
    }
    if (op.op === 'install' && (op.phase === 'starting' || op.phase === 'harvest')) {
      const u = app.updates[op.game_id];
      return u ? 2 * u.download_bytes : 0;
    }
    return 0;
  }

  interface Volume {
    id: string;
    space: DiskSpace;
    labels: string[];
    ops: Operation[];
    need: number;
  }

  const volumes = $derived.by((): Volume[] => {
    const byId = new Map<string, Volume>();
    for (const t of targets) {
      const s = spaces[t.path];
      if (!s) continue;
      const id = s.volume || s.path;
      let v = byId.get(id);
      if (!v) {
        v = { id, space: s, labels: [], ops: [], need: 0 };
        byId.set(id, v);
      }
      if (!v.labels.includes(t.label)) v.labels.push(t.label);
      if (t.op && !v.ops.includes(t.op)) {
        v.ops.push(t.op);
        v.need += stillNeeds(t.op);
      }
    }
    return [...byId.values()];
  });

  const failed = $derived(targets.filter((t) => errors[t.path] && !spaces[t.path]));

  const installed = $derived(
    app.games
      .filter((g) => g.installed)
      .map((g) => ({ g, size: installedSize(g) }))
      .sort((a, b) => (b.size ?? -1) - (a.size ?? -1) || a.g.title.localeCompare(b.g.title)),
  );
  const knownTotal = $derived(installed.reduce((s, i) => s + (i.size ?? 0), 0));
  const unknownCount = $derived(installed.filter((i) => i.size == null).length);
</script>

<section class="card" aria-labelledby="disk-title">
  <h2 id="disk-title">Storage</h2>

  {#if !targets.length}
    <p class="dim small">Loading…</p>
  {/if}

  {#each volumes as v (v.id)}
    {@const used = v.space.total_bytes - v.space.free_bytes}
    {@const usedPct = percent(used, v.space.total_bytes)}
    {@const short = v.need > v.space.free_bytes}
    {@const low = !short && v.space.free_bytes < v.space.total_bytes * 0.05}
    <div class="vol">
      <div class="vol-head">
        <span class="labels">{v.labels.join(' · ')}</span>
        <span class="free"><b>{formatBytes(v.space.free_bytes)}</b> free of {formatBytes(v.space.total_bytes)}</span>
      </div>
      <div
        class="meter"
        role="meter"
        aria-label="Disk space used on the drive holding {v.labels.join(', ')}"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={Math.round(usedPct)}
      >
        <div class="used" class:warn={short || low} style:width="{usedPct}%"></div>
        {#if v.need > 0 && !short}
          <div class="pending" style:left="{usedPct}%" style:width="{percent(v.need, v.space.total_bytes)}%"></div>
        {/if}
      </div>
      <div class="dim small mono path" title={v.space.path}>{v.space.path}</div>
      {#if v.need > 0}
        <div class="dim small">
          The active {v.ops.length === 1 ? 'install still needs' : 'installs still need'} about {formatBytes(v.need)} here.
        </div>
      {/if}
      {#if short}
        <div class="alert" role="alert">
          <span aria-hidden="true">⚠</span>
          Not enough space: {v.ops.map((o) => o.title).join(', ')}
          {v.ops.length === 1 ? 'needs' : 'need'} about {formatBytes(v.need)} more, but only {formatBytes(v.space.free_bytes)} is free. Free
          up space or the install will fail.
        </div>
      {:else if low}
        <div class="alert soft"><span aria-hidden="true">⚠</span> This drive is almost full.</div>
      {/if}
    </div>
  {/each}

  {#each failed as t (t.path)}
    <p class="dim small">Could not read free space for <span class="mono">{t.path}</span>: {errors[t.path]}</p>
  {/each}

  {#if installed.length}
    <h3 class="games-head">
      <span>Installed games</span>
      {#if knownTotal}<span class="total">{formatBytes(knownTotal)}{unknownCount ? '+' : ''}</span>{/if}
    </h3>
    <ul class="games">
      {#each installed as { g, size } (g.game_id)}
        <li>
          <button type="button" class="link" onclick={() => app.select(g.game_id)}>{g.title}</button>
          <span class="dim small">{g.installed?.version}</span>
          <span class="size" title={size == null ? 'Size unknown: the manifest of the installed version could not be read' : undefined}
            >{size == null ? '—' : formatBytes(size)}</span
          >
        </li>
      {/each}
    </ul>
    <p class="dim small">
      {installed.length === 1 ? '1 game' : `${installed.length} games`}{knownTotal ? `, ${formatBytes(knownTotal)} in total` : ''}{unknownCount
        ? `; ${unknownCount === installed.length ? 'no size is known' : `${unknownCount} without a known size`} (manifest unreadable, reinstall to fix)`
        : ''}.
    </p>
  {/if}
</section>

<style>
  .card {
    display: grid;
    gap: 14px;
    padding: 16px 18px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 0;
    align-content: start;
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  h3 {
    margin: 4px 0 0;
    font-size: 0.85rem;
    color: var(--text-dim);
    font-weight: 600;
  }
  .games-head {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .games-head .total {
    color: var(--text-strong);
    font-variant-numeric: tabular-nums;
  }
  .vol {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  .vol-head {
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    gap: 4px 12px;
    font-size: 0.9rem;
  }
  .labels {
    font-weight: 600;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .free {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .free b {
    color: var(--text-strong);
  }
  .meter {
    position: relative;
    height: 10px;
    border-radius: 99px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .used {
    height: 100%;
    background: var(--text-dim);
    opacity: 0.75;
  }
  .used.warn {
    background: var(--danger);
    opacity: 1;
  }
  .pending {
    position: absolute;
    top: 0;
    height: 100%;
    background: repeating-linear-gradient(135deg, var(--chart-net) 0 4px, transparent 4px 7px);
    opacity: 0.8;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .alert {
    padding: 8px 10px;
    border-radius: var(--radius);
    border: 1px solid var(--danger);
    color: var(--text);
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    font-size: 0.85rem;
  }
  .alert.soft {
    border-color: var(--warning);
    background: var(--warning-soft);
  }
  .games {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 4px;
  }
  .games li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .size {
    margin-left: auto;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
    cursor: pointer;
    text-align: left;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .link:hover {
    text-decoration: underline;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 0.8rem;
  }
  p {
    margin: 0;
  }
</style>

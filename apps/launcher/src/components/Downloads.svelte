<script lang="ts">
  import GameImage from './GameImage.svelte';
  import ProgressBar from './ProgressBar.svelte';
  import ThroughputGraph from './ThroughputGraph.svelte';
  import DiskUsage from './DiskUsage.svelte';
  import { app, type Operation } from '../lib/state.svelte';
  import { assetUrl } from '../lib/api';
  import { formatBytes, formatDuration, formatSpeed, seedRatio, share, stepLabel } from '../lib/format';
  import { channelOf, type StreamStats } from '../lib/throughput';

  function iconOf(id: string) {
    const g = app.byId(id);
    return g ? assetUrl(g.asset_base, g.icon) : null;
  }

  function time(t: number) {
    return new Date(t).toLocaleTimeString(undefined, { timeStyle: 'short' });
  }

  // Poll seeding stats and keep the traffic snapshot ticking while this view is mounted.
  $effect(() => app.watchSeeding());
  $effect(() => app.watchTraffic());

  let range = $state<60 | 300>(300);

  const seedOn = $derived(app.settings?.seed ?? false);
  const t = $derived(app.traffic);
  const ops = $derived(app.activeOps);
  const showUpload = $derived(seedOn && app.seeding.games.length > 0);
  const uploadedTotal = $derived(app.seeding.games.reduce((s, g) => s + g.uploaded_bytes, 0));

  const visible = $derived(t.history.filter((b) => b.t >= t.now - range * 1000));

  /** Anything moved within the visible range, or something is running: show the graph. */
  const active = $derived.by(() => {
    if (ops.length) return true;
    const r = t.rates;
    if (r.net > 0 || r.write > 0 || r.read > 0 || r.up > 0) return true;
    return visible.some((b) => b.net > 0 || b.write > 0 || b.read > 0 || b.up > 0);
  });

  /**
   * Peers figures only when other players are or were involved: a direct
   * (server-only) download shows neither an empty peers series nor "0% from peers".
   */
  const peersNow = $derived(ops.some((o) => o.phase === 'download' && o.source === 'peers'));
  const showPeers = $derived(peersNow || t.rates.peers > 0 || visible.some((b) => b.peers > 0));
  const showPeerStats = $derived(peersNow || t.session.bytes.peers > 0);
  /** Reading installed files (verify, harvest) is drawn only when it happens. */
  const showReads = $derived(t.rates.read > 0 || ops.some((o) => channelOf(o.phase) === 'read') || visible.some((b) => b.read > 0));

  const batchShare = $derived(t.batch ? share(t.batch.peers, t.batch.net) : null);
  const sessionShare = $derived(share(t.session.bytes.peers, t.session.bytes.net));

  function stats(op: Operation): StreamStats | undefined {
    return t.streams[op.game_id];
  }

  /** What an operation is doing, and for downloads where from, in plain words. */
  function doing(op: Operation, s: StreamStats | undefined): string {
    if (op.phase === 'starting') return op.op === 'verify' ? 'Preparing to check files' : 'Contacting the server';
    if (op.phase === 'download') {
      if (op.source === 'peers') {
        // The torrent counter is full: the engine now hashes what arrived (no events).
        if (op.total > 0 && op.done >= op.total) return 'Checking the data from other players';
        if (s?.stalled) return 'Waiting for other players (the server takes over if they stay silent)';
        return 'Downloading from other players (BitTorrent)';
      }
      if (s?.stalled) return 'Waiting for the server';
      if (op.op === 'verify') return 'Downloading the damaged parts from the server';
      if (s && s.peerBytes > 0) return 'Downloading the rest from the server';
      return 'Downloading from the server';
    }
    if (op.phase === 'harvest')
      return op.op === 'verify' ? 'Reading the intact parts of damaged files' : 'Reading reusable data from the installed version';
    if (op.phase === 'assemble') {
      if (op.total > 0 && op.done >= op.total && app.settings?.seed) return 'Finishing (checking the files before sharing them)';
      return op.op === 'verify' ? 'Writing the repaired files' : 'Writing game files to disk';
    }
    if (op.phase === 'verify') return 'Reading and checking the installed files';
    return stepLabel(op);
  }

  /** The current rate with what it measures, e.g. "reading 412 MiB/s". */
  function rateText(op: Operation, s: StreamStats): string {
    const ch = channelOf(op.phase);
    const v = formatSpeed(s.rate);
    if (ch === 'net') return op.source === 'peers' ? `${v} from peers` : op.source === 'http' ? `${v} from the server` : v;
    if (ch === 'write') return `writing ${v}`;
    if (ch === 'read') return op.phase === 'verify' ? `checking ${v}` : `reading ${v}`;
    return v;
  }

  /** Longest meaningful ETA among the running operations (they are all done when it ends). */
  const eta = $derived.by(() => {
    let best: { secs: number; op: Operation } | null = null;
    let estimating = false;
    for (const op of ops) {
      const s = stats(op);
      if (!s || !channelOf(op.phase)) continue;
      if (s.eta == null) estimating = true;
      else if (!best || s.eta > best.secs) best = { secs: s.eta, op };
    }
    return { best, estimating };
  });

  const peak = $derived(t.batch ? t.batch.peak : t.session.peak);
  const avg = $derived(t.batch ? t.batch.average : t.session.average);
  const scope = $derived(t.batch ? (ops.length > 1 ? 'active downloads' : 'this download') : 'this session');

  const headline = $derived.by(() => {
    if (!ops.length) return active ? 'No downloads running' : 'Idle';
    const n = `${ops.length} active`;
    const r = t.rates;
    if (r.net > 0) return `${n} · ${formatSpeed(r.net)} down`;
    if (r.read > 0) return `${n} · reading files at ${formatSpeed(r.read)}`;
    if (r.write > 0) return `${n} · writing files at ${formatSpeed(r.write)}`;
    return n;
  });

  const diskSub = $derived.by(() => {
    const r = t.rates;
    if (r.write > 0 && r.read > 0) return `writing ${formatSpeed(r.write)} · reading ${formatSpeed(r.read)}`;
    if (r.read > 0) return 'reading and checking files';
    if (r.write > 0) return 'writing game files';
    const b = t.session.bytes;
    return b.read > 0 ? `${formatBytes(b.write)} written · ${formatBytes(b.read)} read` : `${formatBytes(b.write)} written`;
  });
</script>

<section class="page downloads" aria-labelledby="dl-title">
  <header class="head">
    <h1 id="dl-title">Downloads</h1>
    <span class="status" aria-live="polite">{headline}</span>
  </header>

  {#if active}
    <section class="card monitor" aria-label="Transfer activity">
      <div class="mon-head">
        <ul class="legend" aria-label="Graph legend">
          {#if showPeers}
            <li><span class="key server"></span>Download from the server</li>
            <li><span class="key peers"></span>Download from other players</li>
          {:else}
            <li><span class="key net"></span>Download</li>
          {/if}
          <li><span class="key disk"></span>Writing files</li>
          {#if showReads}<li><span class="key read"></span>Reading and checking files</li>{/if}
          {#if showUpload}<li><span class="key up"></span>Upload to other players</li>{/if}
        </ul>
        <div class="seg" role="group" aria-label="Time range">
          <button type="button" aria-pressed={range === 60} onclick={() => (range = 60)}>1 min</button>
          <button type="button" aria-pressed={range === 300} onclick={() => (range = 300)}>5 min</button>
        </div>
      </div>
      <div class="mon-body">
        <ThroughputGraph traffic={t} {range} {showUpload} {showPeers} {showReads} />
        <dl class="tiles">
          <div class="tile wide">
            <dt><span class="key net"></span>Download speed</dt>
            <dd class="big">{formatSpeed(t.rates.net)}</dd>
            {#if showPeers}
              <dd class="sub">
                {formatSpeed(Math.max(0, t.rates.net - t.rates.peers))} from the server · {formatSpeed(t.rates.peers)} from other players
              </dd>
            {/if}
          </div>
          {#if showPeerStats}
            <div class="tile wide">
              <dt><span class="key peers"></span>From other players</dt>
              <dd>{formatSpeed(t.rates.peers)}</dd>
              <dd class="sub">
                {#if t.batch}
                  {batchShare ?? 'nothing yet'} of {ops.length > 1 ? 'the active downloads' : 'this download'}{sessionShare
                    ? ` · ${sessionShare} of this session (${formatBytes(t.session.bytes.peers)})`
                    : ''}
                {:else}
                  {sessionShare ?? 'nothing'} of this session ({formatBytes(t.session.bytes.peers)})
                {/if}
              </dd>
            </div>
          {/if}
          <div class="tile">
            <dt>Peak</dt>
            <dd>{peak > 0 ? formatSpeed(peak) : '—'}</dd>
            <dd class="sub">{scope}</dd>
          </div>
          <div class="tile">
            <dt>Average</dt>
            <dd>{avg != null ? formatSpeed(avg) : '—'}</dd>
            <dd class="sub">{scope}</dd>
          </div>
          {#if ops.length}
            <div class="tile">
              <dt>Time left</dt>
              {#if eta.best}
                <dd>{formatDuration(eta.best.secs)}</dd>
                <dd class="sub">{stepLabel(eta.best.op).toLowerCase()}{ops.length > 1 ? `, ${eta.best.op.title}` : ''}</dd>
              {:else}
                <dd class="muted">{eta.estimating ? 'Estimating…' : '—'}</dd>
              {/if}
            </div>
          {/if}
          <div class="tile">
            <dt>Downloaded</dt>
            <dd>{formatBytes(t.session.bytes.net)}</dd>
            <dd class="sub">this session</dd>
          </div>
          <div class="tile">
            <dt><span class="key disk"></span>Disk</dt>
            <dd>{formatSpeed(t.rates.write + t.rates.read)}</dd>
            <dd class="sub" title={diskSub}>{diskSub}</dd>
          </div>
          {#if showUpload}
            <div class="tile">
              <dt><span class="key up"></span>Upload</dt>
              <dd>{formatSpeed(t.rates.up)}</dd>
              <dd class="sub">{formatBytes(uploadedTotal)} shared</dd>
            </div>
          {/if}
        </dl>
      </div>
    </section>
  {:else}
    <section class="card idle" aria-label="Session summary">
      <div class="idle-msg">
        <strong>Nothing is downloading.</strong>
        <span class="dim">
          {#if seedOn && app.seeding.games.length}
            No peer has asked for data in the last {range / 60} minutes.
          {:else}
            Install or update a game and its speed shows up here.
          {/if}
        </span>
      </div>
      <dl class="tiles row">
        <div class="tile">
          <dt>Downloaded</dt>
          <dd>{formatBytes(t.session.bytes.net)}</dd>
          <dd class="sub">this session</dd>
        </div>
        <div class="tile">
          <dt>Peak</dt>
          <dd>{t.session.peak > 0 ? formatSpeed(t.session.peak) : '—'}</dd>
          <dd class="sub">this session</dd>
        </div>
        <div class="tile">
          <dt>Average</dt>
          <dd>{t.session.average != null ? formatSpeed(t.session.average) : '—'}</dd>
          <dd class="sub">while downloading</dd>
        </div>
        {#if t.session.bytes.peers > 0}
          <div class="tile">
            <dt>From other players</dt>
            <dd>{formatBytes(t.session.bytes.peers)}</dd>
            <dd class="sub">{sessionShare} of the downloads</dd>
          </div>
        {/if}
        <div class="tile">
          <dt>Written to disk</dt>
          <dd>{formatBytes(t.session.bytes.write)}</dd>
          <dd class="sub">this session</dd>
        </div>
        {#if t.session.bytes.read > 0}
          <div class="tile">
            <dt>Read from disk</dt>
            <dd>{formatBytes(t.session.bytes.read)}</dd>
            <dd class="sub">checked or reused</dd>
          </div>
        {/if}
        {#if seedOn}
          <div class="tile">
            <dt>Uploaded</dt>
            <dd>{formatBytes(uploadedTotal)}</dd>
            <dd class="sub">to other players</dd>
          </div>
        {/if}
      </dl>
    </section>
  {/if}

  <div class="cols">
    <div class="main-col">
      <h2>Active</h2>
      {#if ops.length === 0}
        <p class="dim">Nothing is downloading right now.</p>
      {:else}
        <ul class="list">
          {#each ops as op (op.game_id)}
            {@const s = stats(op)}
            <li class="row">
              <div class="icon"><GameImage src={iconOf(op.game_id)} title={op.title} /></div>
              <div class="info">
                <div class="name">
                  <button type="button" class="link" onclick={() => app.select(op.game_id)}>{op.title}</button>
                  <span class="badge">{op.op === 'install' ? 'Install / update' : 'Verify'}</span>
                  <span class="doing" class:stalled={s?.stalled}>{doing(op, s)}</span>
                </div>
                <ProgressBar {op} />
                {#if s && channelOf(op.phase)}
                  {@const fromPeers = share(s.peerBytes, s.netBytes)}
                  <div class="opstats">
                    <span>{rateText(op, s)}</span>
                    {#if s.eta != null}<span>about {formatDuration(s.eta)} left{op.phase === 'download' ? '' : ' in this step'}</span>{/if}
                    {#if op.phase === 'download'}
                      {#if s.peak > 0}<span class="dim">peak {formatSpeed(s.peak)}</span>{/if}
                      {#if s.average != null}<span class="dim">avg {formatSpeed(s.average)}</span>{/if}
                    {/if}
                    {#if fromPeers}
                      <span class="dim"><span class="key peers"></span>{formatBytes(s.peerBytes)} from other players ({fromPeers} of {formatBytes(s.netBytes)} downloaded)</span>
                    {/if}
                  </div>
                {/if}
              </div>
            </li>
          {/each}
        </ul>
      {/if}

      <h2>Seeding</h2>
      {#if app.seeding.games.length}
        <p class="dim small seed-sum">
          Uploading at <b>{formatSpeed(t.rates.up)}</b> · {formatBytes(uploadedTotal)} shared so far
          {#if app.seeding.port != null}· listening for peers on TCP port <span class="mono">{app.seeding.port}</span>{/if}
        </p>
        <ul class="list">
          {#each app.seeding.games as s (s.game_id)}
            <li class="row">
              <div class="icon"><GameImage src={iconOf(s.game_id)} title={app.byId(s.game_id)?.title ?? s.game_id} /></div>
              <div class="info">
                <div class="name">
                  <button type="button" class="link" onclick={() => app.select(s.game_id)}>{app.byId(s.game_id)?.title ?? s.game_id}</button>
                </div>
                <div class="dim small">
                  Uploaded {formatBytes(s.uploaded_bytes)} (ratio {seedRatio(s)}) · {formatBytes(s.total_bytes)} shared
                </div>
              </div>
              <button type="button" class="btn small" onclick={() => app.stopSeeding(s.game_id)}>Stop</button>
            </li>
          {/each}
        </ul>
      {:else if seedOn}
        <p class="dim">Seeding is on, but no game is being seeded yet. Installed games are checked and shared in the background.</p>
      {:else}
        <p class="dim">
          Seeding is off.
          <button type="button" class="link inline" onclick={() => (app.view = 'settings')}>Turn it on in Settings</button>
          to share the games you have installed with other players.
        </p>
      {/if}

      {#if app.history.length}
        <h2>Recent</h2>
        <ul class="list">
          {#each app.history as h (h.at + h.game_id)}
            <li class="row done" class:failed={!h.ok}>
              <div class="icon"><GameImage src={iconOf(h.game_id)} title={h.title} /></div>
              <div class="info">
                <div class="msg">{h.message}</div>
                <div class="dim small">{h.ok ? 'Completed' : 'Failed'} at {time(h.at)}</div>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <aside class="side-col" aria-label="Storage">
      <DiskUsage />
    </aside>
  </div>
</section>

<style>
  .downloads {
    container-type: inline-size;
    max-width: 1280px;
  }
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 16px;
    margin: 0 0 16px;
  }
  h1 {
    margin: 0;
  }
  .status {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  h2 {
    font-size: 1rem;
    margin: 24px 0 10px;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .main-col > h2:first-child {
    margin-top: 0;
  }
  .card {
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 16px;
    min-width: 0;
  }

  /* ---- monitor ---- */
  .monitor {
    display: grid;
    gap: 10px;
  }
  .mon-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 16px;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 16px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: 0.82rem;
    color: var(--text-dim);
  }
  .legend li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .key {
    display: inline-block;
    width: 14px;
    height: 3px;
    border-radius: 2px;
    flex: none;
    vertical-align: middle;
  }
  .key.net {
    height: 9px;
    background: var(--chart-net);
  }
  .key.server {
    height: 9px;
    background: color-mix(in srgb, var(--chart-net) 45%, transparent);
    border-top: 2px solid var(--chart-net);
  }
  .key.peers {
    height: 9px;
    background: repeating-linear-gradient(135deg, var(--chart-peers) 0 2px, transparent 2px 4px);
    outline: 1px solid var(--chart-peers);
    outline-offset: -1px;
  }
  .key.disk {
    background: var(--chart-disk);
  }
  .key.read {
    background: repeating-linear-gradient(90deg, var(--chart-disk) 0 2px, transparent 2px 5px);
  }
  .opstats .key {
    margin-right: 5px;
  }
  .key.up {
    background: repeating-linear-gradient(90deg, var(--chart-up) 0 4px, transparent 4px 6px);
  }
  dt .key {
    margin-right: 6px;
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
  }
  .seg button {
    padding: 3px 10px;
    border: 0;
    background: none;
    color: var(--text-dim);
    font: inherit;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
  }
  .seg button + button {
    border-left: 1px solid var(--border);
  }
  .seg button[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--text-strong);
  }
  .mon-body {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 300px;
    gap: 16px;
    align-items: start;
  }
  @container (max-width: 820px) {
    .mon-body {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    margin: 0;
  }
  @container (max-width: 820px) {
    .tiles {
      grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    }
  }
  .tiles.row {
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  }
  .tile {
    padding: 8px 10px;
    border-radius: var(--radius);
    background: var(--surface-2);
    min-width: 0;
  }
  .tile.wide {
    grid-column: 1 / -1;
  }
  dt {
    font-size: 0.75rem;
    color: var(--text-dim);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    display: flex;
    align-items: center;
  }
  dd {
    margin: 2px 0 0;
    color: var(--text-strong);
    font-weight: 700;
    font-size: 1.05rem;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  dd.big {
    font-size: 1.6rem;
    line-height: 1.15;
  }
  dd.sub {
    margin: 0;
    font-size: 0.75rem;
    font-weight: 400;
    color: var(--text-dim);
  }
  dd.muted {
    color: var(--text-dim);
    font-weight: 600;
  }
  .idle {
    display: grid;
    gap: 12px;
  }
  .idle-msg {
    display: grid;
    gap: 2px;
  }

  /* ---- columns ---- */
  .cols {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 340px;
    gap: 20px;
    margin-top: 24px;
    align-items: start;
  }
  @container (max-width: 900px) {
    .cols {
      grid-template-columns: minmax(0, 1fr);
    }
  }
  .side-col {
    min-width: 0;
  }

  /* ---- lists ---- */
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 10px;
  }
  .row {
    display: flex;
    gap: 14px;
    align-items: center;
    padding: 12px 14px;
    background: var(--surface-1);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
  }
  .row.failed {
    border-color: var(--danger);
  }
  .icon {
    width: 44px;
    height: 44px;
    flex: none;
    font-size: 1.2rem;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 6px;
  }
  .name {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .doing {
    margin-left: auto;
    font-size: 0.82rem;
    color: var(--text-dim);
  }
  .doing.stalled {
    color: var(--warning);
  }
  .opstats {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 14px;
    font-size: 0.8rem;
    font-variant-numeric: tabular-nums;
  }
  .opstats > :first-child {
    font-weight: 700;
    color: var(--text-strong);
  }
  .seed-sum {
    margin: -2px 0 10px;
  }
  .seed-sum b {
    color: var(--text-strong);
  }
  .link {
    padding: 0;
    border: 0;
    background: none;
    color: var(--text-strong);
    font: inherit;
    font-weight: 700;
    cursor: pointer;
    text-align: left;
  }
  .link:hover {
    text-decoration: underline;
  }
  .link.inline {
    color: var(--accent);
    font-weight: 600;
  }
  .badge {
    font-size: 0.72rem;
    padding: 1px 8px;
    border-radius: 99px;
    background: var(--surface-3);
    color: var(--text-dim);
  }
  .msg {
    overflow-wrap: anywhere;
  }
  .dim {
    color: var(--text-dim);
  }
  .small {
    font-size: 0.8rem;
  }
</style>

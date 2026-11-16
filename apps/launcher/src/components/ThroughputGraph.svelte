<script lang="ts">
  // Hand-rolled SVG throughput graph. Network download is a filled area whose
  // top edge is the total download speed; when other players contributed, the
  // area is stacked: bytes from peers at the bottom (hatched), bytes from the
  // server above it (plain fill). Disk writes are a solid line, disk reads
  // (checking or reusing installed files) a dotted line and, when seeding,
  // upload a dashed line. One point per second; hover or arrow keys show the
  // exact values of a second.
  import { STEP, type Bucket, type TrafficSnapshot } from '../lib/throughput';
  import { formatSpeed } from '../lib/format';

  let {
    traffic,
    range = 300,
    showUpload = false,
    showPeers = false,
    showReads = false,
  }: { traffic: TrafficSnapshot; range?: number; showUpload?: boolean; showPeers?: boolean; showReads?: boolean } =
    $props();

  const HEIGHT = 220;
  const M = { top: 12, right: 12, bottom: 24, left: 74 };
  const UNITS = ['B', 'KiB', 'MiB', 'GiB'];

  let width = $state(0);
  let hover = $state<number | null>(null);

  interface Point {
    t: number; // bucket end
    net: number;
    peers: number;
    write: number;
    read: number;
    up: number | null; // null: upload not reported yet for this second
  }

  /** One point per second of the range, ending at the last full second. */
  const points = $derived.by((): Point[] => {
    const hist = traffic.history;
    const lastEnd = hist.length ? hist[hist.length - 1].t + STEP : Math.floor(traffic.now / STEP) * STEP;
    const byT = new Map<number, Bucket>();
    for (const b of hist) byT.set(b.t, b);
    const out: Point[] = [];
    for (let i = range - 1; i >= 0; i--) {
      const start = lastEnd - (i + 1) * STEP;
      const b = byT.get(start);
      const net = b?.net ?? 0;
      out.push({
        t: start + STEP,
        net,
        peers: Math.min(net, b?.peers ?? 0),
        write: b?.write ?? 0,
        read: b?.read ?? 0,
        up: start + STEP <= traffic.upUntil ? (b?.up ?? 0) : null,
      });
    }
    return out;
  });

  /** A round axis maximum and 3–5 ticks in one binary unit. */
  const scale = $derived.by(() => {
    let max = 0;
    for (const p of points) max = Math.max(max, p.net, p.write, p.read, showUpload ? (p.up ?? 0) : 0);
    max = Math.max(max * 1.08, 64 * 1024);
    let u = 0;
    while (u < UNITS.length - 1 && max >= 1024 ** (u + 1)) u++;
    const inUnit = max / 1024 ** u;
    const raw = inUnit / 4;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 2.5, 5, 10].map((f) => f * mag).find((s) => s >= raw) ?? 10 * mag;
    const top = Math.ceil(inUnit / step) * step;
    const ticks: { v: number; label: string }[] = [];
    for (let v = 0; v <= top + step / 1000; v += step) {
      const shown = Number.isInteger(v) ? String(v) : v.toFixed(step < 1 ? 2 : 1).replace(/\.?0+$/, '');
      ticks.push({ v: v * 1024 ** u, label: `${shown} ${UNITS[u]}/s` });
    }
    return { max: top * 1024 ** u, ticks };
  });

  const plotW = $derived(Math.max(0, width - M.left - M.right));
  const plotH = HEIGHT - M.top - M.bottom;
  const x = (i: number) => M.left + (points.length > 1 ? (i / (points.length - 1)) * plotW : 0);
  const y = (v: number) => M.top + plotH - (v / scale.max) * plotH;
  const base = M.top + plotH;

  function line(values: (number | null)[]): string {
    let d = '';
    let pen = false;
    values.forEach((v, i) => {
      if (v == null) {
        pen = false;
        return;
      }
      d += `${pen ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`;
      pen = true;
    });
    return d;
  }

  /** Closed band between a lower and an upper series (both full length). */
  function band(lower: number[], upper: number[]): string {
    if (!upper.length) return '';
    let d = '';
    upper.forEach((v, i) => (d += `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`));
    for (let i = lower.length - 1; i >= 0; i--) d += `L${x(i).toFixed(1)},${y(lower[i]).toFixed(1)}`;
    return d + 'Z';
  }

  const netLine = $derived(line(points.map((p) => p.net)));
  const peersVals = $derived(points.map((p) => (showPeers ? p.peers : 0)));
  const serverArea = $derived(band(peersVals, points.map((p) => p.net)));
  const peersArea = $derived(showPeers ? band(points.map(() => 0), peersVals) : '');
  const peersLine = $derived(showPeers ? line(peersVals) : '');
  const writeLine = $derived(line(points.map((p) => p.write)));
  const readLine = $derived(showReads ? line(points.map((p) => p.read)) : '');
  const upLine = $derived(showUpload ? line(points.map((p) => p.up)) : '');

  /** Time labels relative to now, every minute (or 15 s for a 1-minute range). */
  const xTicks = $derived.by(() => {
    const every = range <= 60 ? 15 : range <= 180 ? 30 : 60;
    const minGap = 56;
    const out: { x: number; label: string }[] = [];
    for (let ago = range - (range % every); ago >= 0; ago -= every) {
      const i = points.length - 1 - ago;
      if (i < 0) continue;
      const label = ago === 0 ? 'now' : ago < 60 || ago % 60 ? `−${ago} s` : `−${ago / 60} min`;
      const px = x(i);
      if (out.length && px - out[out.length - 1].x < minGap) continue;
      out.push({ x: px, label });
    }
    return out;
  });

  function onMove(e: PointerEvent) {
    const svg = e.currentTarget as SVGSVGElement;
    const r = svg.getBoundingClientRect();
    const px = e.clientX - r.left;
    if (px < M.left - 8 || plotW <= 0) {
      hover = null;
      return;
    }
    hover = Math.max(0, Math.min(points.length - 1, Math.round(((px - M.left) / plotW) * (points.length - 1))));
  }

  function onKey(e: KeyboardEvent) {
    const last = points.length - 1;
    if (e.key === 'ArrowLeft') hover = Math.max(0, (hover ?? last + 1) - (e.shiftKey ? 10 : 1));
    else if (e.key === 'ArrowRight') hover = Math.min(last, (hover ?? last) + (e.shiftKey ? 10 : 1));
    else if (e.key === 'Home') hover = 0;
    else if (e.key === 'End') hover = last;
    else if (e.key === 'Escape') hover = null;
    else return;
    e.preventDefault();
  }

  const hp = $derived(hover != null ? points[hover] : null);
  const tipLeft = $derived(hover != null ? x(hover) > M.left + plotW * 0.6 : false);

  function clock(t: number) {
    return new Date(t).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }

  const summary = $derived.by(() => {
    let peak = 0;
    let net = 0;
    let peers = 0;
    for (const p of points) {
      peak = Math.max(peak, p.net);
      net += p.net;
      peers += p.peers;
    }
    const from = showPeers && net > 0 ? ` ${Math.round((peers / net) * 100)}% of it from other players.` : '';
    return `Throughput over the last ${range / 60} minutes. Peak download ${formatSpeed(peak)}.${from} Use the arrow keys to read individual seconds.`;
  });
</script>

<div class="graph" bind:clientWidth={width}>
  {#if width > 0}
    <!-- A focusable picture: arrow keys move the readout cursor. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <svg
      {width}
      height={HEIGHT}
      role="img"
      aria-label={summary}
      tabindex="0"
      onpointermove={onMove}
      onpointerleave={() => (hover = null)}
      onkeydown={onKey}
      onblur={() => (hover = null)}
    >
      <defs>
        <linearGradient id="net-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stop-color="var(--chart-net)" stop-opacity="0.32" />
          <stop offset="100%" stop-color="var(--chart-net)" stop-opacity="0.06" />
        </linearGradient>
        <!-- Peers: diagonal hatch, so the two network layers differ without colour. -->
        <pattern id="peers-hatch" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)">
          <rect width="6" height="6" fill="var(--chart-peers)" fill-opacity="0.16" />
          <line x1="0" y1="0" x2="0" y2="6" stroke="var(--chart-peers)" stroke-width="2.4" stroke-opacity="0.75" />
        </pattern>
      </defs>

      {#each scale.ticks as tk (tk.v)}
        <line class="grid" x1={M.left} x2={M.left + plotW} y1={y(tk.v)} y2={y(tk.v)} />
        <text class="ylab" x={M.left - 8} y={y(tk.v)} dy="0.32em">{tk.label}</text>
      {/each}
      {#each xTicks as tk (tk.label)}
        <text class="xlab" x={tk.x} y={HEIGHT - 6} text-anchor={tk.label === 'now' ? 'end' : 'middle'}>{tk.label}</text>
      {/each}

      {#if serverArea}<path class="server-area" d={serverArea} />{/if}
      {#if peersArea}<path class="peers-area" d={peersArea} />{/if}
      {#if peersLine}<path class="series peers" d={peersLine} />{/if}
      <path class="series net" d={netLine} />
      <path class="series write" d={writeLine} />
      {#if readLine}<path class="series read" d={readLine} />{/if}
      {#if upLine}<path class="series up" d={upLine} />{/if}

      {#if hp && hover != null}
        <line class="cursor" x1={x(hover)} x2={x(hover)} y1={M.top} y2={base} />
        <circle class="dot net" cx={x(hover)} cy={y(hp.net)} r="4" />
        {#if showPeers && hp.peers > 0}<circle class="dot peers" cx={x(hover)} cy={y(hp.peers)} r="3.5" />{/if}
        <circle class="dot disk" cx={x(hover)} cy={y(hp.write)} r="4" />
        {#if showReads}<circle class="dot disk hollow" cx={x(hover)} cy={y(hp.read)} r="3.5" />{/if}
        {#if showUpload && hp.up != null}<circle class="dot up" cx={x(hover)} cy={y(hp.up)} r="4" />{/if}
      {/if}
    </svg>

    {#if hp && hover != null}
      <div class="tip" class:left={tipLeft} style:left="{x(hover)}px" aria-live="polite">
        <div class="when">{clock(hp.t)}</div>
        <div class="tl"><span class="sw net"></span>Download<b>{formatSpeed(hp.net)}</b></div>
        {#if showPeers}
          <div class="tl sub"><span class="sw server"></span>from the server<b>{formatSpeed(hp.net - hp.peers)}</b></div>
          <div class="tl sub"><span class="sw peers"></span>from other players<b>{formatSpeed(hp.peers)}</b></div>
        {/if}
        <div class="tl"><span class="sw disk"></span>Disk writes<b>{formatSpeed(hp.write)}</b></div>
        {#if showReads || hp.read > 0}
          <div class="tl"><span class="sw read"></span>Disk reads<b>{formatSpeed(hp.read)}</b></div>
        {/if}
        {#if showUpload}
          <div class="tl"><span class="sw up"></span>Upload<b>{hp.up == null ? '…' : formatSpeed(hp.up)}</b></div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .graph {
    position: relative;
    min-width: 0;
    height: 220px;
  }
  svg {
    display: block;
    overflow: visible;
    touch-action: pan-y;
    border-radius: var(--radius);
  }
  svg:focus-visible {
    outline-offset: 4px;
  }
  .grid {
    stroke: var(--chart-grid);
    stroke-width: 1;
  }
  .ylab,
  .xlab {
    fill: var(--text-dim);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .ylab {
    text-anchor: end;
  }
  .server-area {
    fill: url(#net-fill);
  }
  .peers-area {
    fill: url(#peers-hatch);
  }
  .series {
    fill: none;
    stroke-width: 2;
    stroke-linejoin: round;
    stroke-linecap: round;
  }
  .series.net {
    stroke: var(--chart-net);
  }
  .series.peers {
    stroke: var(--chart-peers);
    stroke-width: 1.25;
  }
  .series.write {
    stroke: var(--chart-disk);
    stroke-width: 1.75;
  }
  .series.read {
    stroke: var(--chart-disk);
    stroke-width: 2;
    stroke-dasharray: 0.5 4;
  }
  .series.up {
    stroke: var(--chart-up);
    stroke-width: 1.75;
    stroke-dasharray: 5 4;
  }
  .cursor {
    stroke: var(--text-dim);
    stroke-width: 1;
    stroke-dasharray: 2 3;
  }
  .dot {
    stroke: var(--surface-1);
    stroke-width: 2;
  }
  .dot.net {
    fill: var(--chart-net);
  }
  .dot.peers {
    fill: var(--chart-peers);
  }
  .dot.disk {
    fill: var(--chart-disk);
  }
  .dot.disk.hollow {
    fill: var(--surface-1);
    stroke: var(--chart-disk);
  }
  .dot.up {
    fill: var(--chart-up);
  }
  .tip {
    position: absolute;
    top: 8px;
    transform: translateX(12px);
    min-width: 200px;
    padding: 8px 10px;
    background: var(--surface-0);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: var(--shadow-lg);
    font-size: 0.8rem;
    pointer-events: none;
    display: grid;
    gap: 3px;
    z-index: 2;
  }
  .tip.left {
    transform: translateX(calc(-100% - 12px));
  }
  .when {
    color: var(--text-dim);
    font-variant-numeric: tabular-nums;
  }
  .tl {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .tl.sub {
    padding-left: 12px;
    color: var(--text-dim);
  }
  .tl b {
    margin-left: auto;
    padding-left: 12px;
    color: var(--text-strong);
    font-variant-numeric: tabular-nums;
  }
  .sw {
    width: 10px;
    height: 3px;
    border-radius: 2px;
    flex: none;
  }
  .sw.net {
    background: var(--chart-net);
    height: 8px;
  }
  .sw.server {
    height: 8px;
    background: color-mix(in srgb, var(--chart-net) 45%, transparent);
  }
  .sw.peers {
    height: 8px;
    background: repeating-linear-gradient(135deg, var(--chart-peers) 0 2px, transparent 2px 4px);
    outline: 1px solid var(--chart-peers);
    outline-offset: -1px;
  }
  .sw.disk {
    background: var(--chart-disk);
  }
  .sw.read {
    background: repeating-linear-gradient(90deg, var(--chart-disk) 0 2px, transparent 2px 4px);
  }
  .sw.up {
    background: repeating-linear-gradient(90deg, var(--chart-up) 0 4px, transparent 4px 6px);
  }
</style>

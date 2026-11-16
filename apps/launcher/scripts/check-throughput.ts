// Assertions for the rate math in src/lib/throughput.ts.
// Run: deno run scripts/check-throughput.ts   (no permissions needed)

import assert from 'node:assert/strict';
import { Meter, RateSeries, STEP, channelOf, HISTORY_SECONDS, type Source } from '../src/lib/throughput.ts';

const MiB = 1024 * 1024;
const T0 = 1_700_000_000_000; // a whole second
let checks = 0;
function near(actual: number, expected: number, tol: number, what: string) {
  checks++;
  assert.ok(Math.abs(actual - expected) <= tol, `${what}: expected ~${expected}, got ${actual}`);
}
function ok(cond: boolean, what: string) {
  checks++;
  assert.ok(cond, what);
}

/** Drive one phase at `rate` bytes/s for `secs` seconds with events every 100 ms. */
function feed(
  m: Meter,
  key: string,
  phase: string,
  total: number,
  rate: number,
  from: number,
  secs: number,
  done0 = 0,
  source: Source | null = phase === 'download' ? 'http' : null,
) {
  let done = done0;
  let t = from;
  for (let i = 1; i <= secs * 10; i++) {
    t = from + i * 100;
    done = Math.min(total, done + rate / 10);
    m.progress(key, phase, done, total, t, source);
    if (i % 10 === 0) m.tick(t);
  }
  return { t, done };
}

// --- phase → channel: only the download phase is network ---
ok(channelOf('download') === 'net', 'download is network');
ok(channelOf('assemble') === 'write', 'assemble is disk write');
ok(channelOf('verify') === 'read' && channelOf('harvest') === 'read', 'verify/harvest are disk reads');
ok(channelOf('done') === null && channelOf('starting') === null, 'done/starting move nothing');

// --- RateSeries: spreading and summing ---
{
  const s = new RateSeries(10);
  s.advance(T0);
  s.add('net', 3000, T0, T0 + 3000); // 1000 B/s over three buckets
  near(s.buckets[0].net, 1000, 1e-6, 'spread bucket 0');
  near(s.buckets[2].net, 1000, 1e-6, 'spread bucket 2');
  near(s.totals.net, 3000, 0, 'series total');
  near(s.rate('net', T0 + 3000, 2000), 1000, 1e-6, 'rate over closed buckets');
  // Gap longer than the ring: everything is zero, nothing throws.
  s.advance(T0 + 60_000);
  ok(s.buckets.length === 10 && s.buckets.every((b) => b.net === 0), 'long gap zero-fills the ring');
  near(s.rate('net', T0 + 60_000, 4000), 0, 0, 'rate after gap');
}

// --- steady download: smoothed rate, peak, average, ETA ---
const m = new Meter();
m.start('a', T0);
const total = 200 * MiB;
let { t, done } = feed(m, 'a', 'download', total, 10 * MiB, T0, 8);
let snap = m.snapshot(t);
near(snap.rates.net, 10 * MiB, 0.02 * MiB, 'steady 10 MiB/s');
near(snap.streams.a.rate, 10 * MiB, 0.02 * MiB, 'per-op rate');
near(snap.rates.write, 0, 0, 'no disk writes while downloading');
near(snap.streams.a.peak, 10 * MiB, 0.05 * MiB, 'peak');
near(snap.streams.a.average ?? 0, 10 * MiB, 0.2 * MiB, 'average');
near(snap.streams.a.eta ?? -1, (total - done) / (10 * MiB), 0.5, 'eta');
near(snap.session.bytes.net, done, 1, 'session total = bytes reported');
ok(snap.batch !== null && Math.abs(snap.batch.net - done) < 1, 'batch total');

// --- no events for a while: the rate decays instead of freezing ---
snap = m.snapshot(t + 2000);
ok(snap.rates.net < 6 * MiB && snap.rates.net > 3 * MiB, `half-decayed after 2 s (${snap.rates.net})`);
snap = m.snapshot(t + 4500);
near(snap.rates.net, 0, 0, 'zero after the window');
ok(snap.streams.a.eta === null, 'no ETA while nothing moves');
m.tick(t + 6000);
snap = m.snapshot(t + 6000);
ok(snap.streams.a.stalled, 'stalled after 5 s without bytes');

// --- counter goes backwards (failed attempt un-counted): no negative, no spike ---
t += 6000;
const before = m.snapshot(t).session.bytes.net;
m.progress('a', 'download', done - 8 * MiB, total, t + 100, 'http');
snap = m.snapshot(t + 100);
near(snap.session.bytes.net, before, 0, 'backwards counter adds nothing');
ok(snap.rates.net >= 0, 'rate never negative');
({ t, done } = feed(m, 'a', 'download', total, 5 * MiB, t + 100, 5, done - 8 * MiB));
snap = m.snapshot(t);
near(snap.rates.net, 5 * MiB, 0.05 * MiB, 'resumes at 5 MiB/s after rebase');
ok(!snap.streams.a.stalled, 'not stalled once bytes flow');
near(snap.streams.a.peak, 10 * MiB, 0.05 * MiB, 'peak is kept');

// --- a changed total with the same source is NOT a restart (no heuristic any more) ---
{
  const before2 = m.snapshot(t).session.bytes.net;
  m.progress('a', 'download', done + 1 * MiB, total + 10 * MiB, t + 100, 'http');
  near(m.snapshot(t + 100).session.bytes.net - before2, 1 * MiB, 1, 'same source, new total: only the delta counts');
  ({ t, done } = feed(m, 'a', 'download', total + 10 * MiB, 5 * MiB, t + 100, 1, done + 1 * MiB));
}

// --- phase change to assemble: disk writes, network decays ---
const writeTotal = 300 * MiB;
({ t } = feed(m, 'a', 'assemble', writeTotal, 100 * MiB, t, 3));
snap = m.snapshot(t);
near(snap.rates.write, 100 * MiB, 1 * MiB, 'disk write rate');
near(snap.streams.a.rate, 100 * MiB, 1 * MiB, 'op rate follows the phase');
near(snap.rates.net, 0, 0, 'network gone after the download phase');
const netBefore = snap.session.bytes.net;
// The final "done" event repeats the full size; it must not count as bytes.
m.progress('a', 'done', writeTotal, writeTotal, t + 50);
snap = m.snapshot(t + 50);
near(snap.session.bytes.net, netBefore, 0, '"done" phase adds no network bytes');
m.end('a', t + 60);
snap = m.snapshot(t + 60);
ok(snap.batch === null && Object.keys(snap.streams).length === 0, 'batch ends with the last op');
ok(snap.session.peak >= 10 * MiB - 0.05 * MiB, 'session peak survives');
ok(!m.idle(t + 60) && m.idle(t + 10_000), 'idle once rates have decayed');

// --- two operations at once: rates add up ---
{
  const m2 = new Meter();
  m2.start('x', T0);
  m2.start('y', T0);
  for (let i = 1; i <= 50; i++) {
    const tt = T0 + i * 100;
    m2.progress('x', 'download', i * 0.3 * MiB, 1000 * MiB, tt); // 3 MiB/s
    m2.progress('y', 'download', i * 0.2 * MiB, 1000 * MiB, tt); // 2 MiB/s
    if (i % 10 === 0) m2.tick(tt);
  }
  const s2 = m2.snapshot(T0 + 5000);
  near(s2.rates.net, 5 * MiB, 0.05 * MiB, 'concurrent rates add');
  near(s2.batch?.peak ?? 0, 5 * MiB, 0.1 * MiB, 'batch peak is the combined rate');
  near(s2.batch?.average ?? 0, 5 * MiB, 0.1 * MiB, 'batch average uses wall time, not op time');
  near(s2.streams.x.rate, 3 * MiB, 0.05 * MiB, 'per-op rate x');
  near(s2.streams.y.rate, 2 * MiB, 0.05 * MiB, 'per-op rate y');
}

// --- upload counters polled every 3 s: spread, not spiky ---
{
  const m3 = new Meter();
  for (let i = 0; i <= 6; i++) m3.counter('up:g', i * 3 * 300 * 1024, T0 + i * 3000); // 300 KiB/s
  const s3 = m3.snapshot(T0 + 18_500);
  near(s3.rates.up, 300 * 1024, 1024, 'upload rate from polled counter');
  const full = s3.history.filter((b) => b.t >= T0 && b.t + STEP <= T0 + 18_000);
  ok(full.every((b) => Math.abs(b.up - 300 * 1024) < 1), 'upload spread evenly across buckets');
  // Seed restarted: counter drops to 0. No negative bytes.
  m3.counter('up:g', 0, T0 + 21_000);
  near(m3.snapshot(T0 + 21_000).session.bytes.up, 6 * 3 * 300 * 1024, 1, 'counter reset adds nothing');
  // Polling stopped: rate decays from now.
  near(m3.snapshot(T0 + 60_000).rates.up, 0, 0, 'upload decays when polls stop');
  ok(m3.snapshot(T0 + 60_000).history.length === 60, 'one closed bucket per second since the first sample');
  ok(m3.snapshot(T0 + 900_000).history.length === HISTORY_SECONDS, 'history capped');
}

// --- download source: peers (torrent totals), then HTTP for the rest (own, smaller total) ---
{
  const ms = new Meter();
  ms.start('g', T0);
  const torrentTotal = 100 * MiB;
  let { t: tt, done: pd } = feed(ms, 'g', 'download', torrentTotal, 8 * MiB, T0, 6, 0, 'peers');
  let sn = ms.snapshot(tt);
  ok(sn.streams.g.source === 'peers', 'stream reports source peers');
  near(sn.rates.net, 8 * MiB, 0.05 * MiB, 'peers: network rate');
  near(sn.rates.peers, 8 * MiB, 0.05 * MiB, 'peers: peers rate = network rate');
  near(sn.streams.g.peerBytes, pd, 1, 'peers: op peer bytes');
  near(sn.session.bytes.peers, sn.session.bytes.net, 1, 'peers: session peers = session net');
  // The swarm stalls for 2 s, then HTTP takes over with its own total.
  tt += 2000;
  ms.tick(tt);
  const httpTotal = torrentTotal - pd;
  ms.progress('g', 'download', 0, httpTotal, tt + 100, 'http');
  ({ t: tt } = feed(ms, 'g', 'download', httpTotal, 4 * MiB, tt + 100, 2, 0, 'http'));
  sn = ms.snapshot(tt);
  ok(sn.streams.g.source === 'http', 'stream reports source http');
  near(sn.streams.g.rate, 4 * MiB, 0.05 * MiB, 'HTTP counter measured from its own zero, not mixed with peers');
  near(sn.rates.peers, 0, 0, 'no peers rate while HTTP downloads');
  near(sn.streams.g.peerBytes, pd, 1, 'op peer bytes kept after the switch');
  near(sn.streams.g.netBytes, pd + 8 * MiB, 1, 'op network bytes = peers + HTTP');
  near(sn.batch?.peers ?? -1, pd, 1, 'batch peer bytes');
  near(sn.batch?.net ?? -1, pd + 8 * MiB, 1, 'batch network bytes');
  near(sn.session.bytes.peers / sn.session.bytes.net, pd / (pd + 8 * MiB), 1e-9, 'session share from peers');
  const peersBuckets = sn.history.filter((b) => b.t >= T0 + 1000 && b.t + STEP <= T0 + 6000);
  ok(peersBuckets.length === 5 && peersBuckets.every((b) => b.peers > 0 && Math.abs(b.peers - b.net) < 1), 'peers buckets: all network bytes from peers');
  const httpBuckets = sn.history.filter((b) => b.t >= T0 + 9000);
  ok(httpBuckets.length > 0 && httpBuckets.every((b) => b.peers === 0 && b.net > 0), 'HTTP buckets: no peers bytes');
}

// --- source change with the same total: still a new counter ---
{
  const ms = new Meter();
  ms.start('h', T0);
  const tot = 100 * MiB;
  let { t: tt } = feed(ms, 'h', 'download', tot, 10 * MiB, T0, 3, 0, 'peers');
  // Peers stop at 30 MiB; HTTP restarts at 0 with an equal total (swarm delivered nothing usable).
  ms.progress('h', 'download', 0, tot, tt + 100, 'http');
  const before = ms.snapshot(tt + 100).session.bytes.net;
  ({ t: tt } = feed(ms, 'h', 'download', tot, 2 * MiB, tt + 100, 2, 0, 'http'));
  const sn = ms.snapshot(tt);
  near(sn.session.bytes.net - before, 4 * MiB, 1, 'HTTP bytes counted from zero after the source change');
  near(sn.streams.h.rate, 2 * MiB, 0.05 * MiB, 'rate of the new source only');
  near(sn.streams.h.peerBytes, 30 * MiB, 1, 'peer bytes unchanged');
}

// --- direct transport: nothing from peers anywhere ---
{
  const md = new Meter();
  md.start('d', T0);
  const { t: tt } = feed(md, 'd', 'download', 50 * MiB, 5 * MiB, T0, 4, 0, 'http');
  const sn = md.snapshot(tt);
  ok(sn.session.bytes.peers === 0 && sn.batch?.peers === 0 && sn.rates.peers === 0, 'direct: no peer bytes or rate');
  ok(sn.streams.d.peerBytes === 0 && sn.history.every((b) => b.peers === 0), 'direct: no peers series');
}

// --- verify, then a repair: harvest, HTTP download, assemble (byte progress for every step) ---
{
  const mv = new Meter();
  mv.start('v', T0);
  const size = 2048 * MiB;
  mv.progress('v', 'verify', 0, size, T0, null);
  let { t: tt, done: vd } = feed(mv, 'v', 'verify', size, 400 * MiB, T0, 4);
  let sn = mv.snapshot(tt);
  near(sn.rates.read, 400 * MiB, 2 * MiB, 'verify: disk read rate');
  near(sn.streams.v.rate, 400 * MiB, 2 * MiB, 'verify: op rate');
  near(sn.rates.net + sn.rates.write, 0, 0, 'verify: no network, no writes');
  near(sn.streams.v.eta ?? -1, (size - vd) / (400 * MiB), 0.2, 'verify: time left');
  ok(!sn.streams.v.stalled, 'verify: never "stalled"');
  ({ t: tt, done: vd } = feed(mv, 'v', 'verify', size, 400 * MiB, tt, 2, vd));
  ok(vd === size, 'verify reached its total');
  const checked = mv.snapshot(tt).session.bytes.read;
  near(checked, size, 1, 'verify: bytes checked = installed size');
  // Harvest of the damaged files (bytes, not a single event with total 0).
  const hv = 1200 * MiB;
  mv.progress('v', 'harvest', 0, hv, tt + 10, null);
  let hd: number;
  ({ t: tt, done: hd } = feed(mv, 'v', 'harvest', hv, 150 * MiB, tt + 10, 4));
  sn = mv.snapshot(tt);
  ok(sn.streams.v.phase === 'harvest', 'harvest: phase');
  near(sn.streams.v.rate, 150 * MiB, 1 * MiB, 'harvest: read rate from its own zero');
  near(sn.streams.v.eta ?? -1, (hv - hd) / (150 * MiB), 0.3, 'harvest: time left');
  ({ t: tt } = feed(mv, 'v', 'harvest', hv, 150 * MiB, tt, 4, hd));
  mv.progress('v', 'download', 0, 20 * MiB, tt + 10, 'http');
  ({ t: tt } = feed(mv, 'v', 'download', 20 * MiB, 5 * MiB, tt + 10, 2));
  ({ t: tt } = feed(mv, 'v', 'assemble', 40 * MiB, 100 * MiB, tt, 1));
  sn = mv.snapshot(tt);
  near(sn.session.bytes.read, size + hv, 1, 'repair: read = verify + harvest');
  near(sn.session.bytes.net, 10 * MiB, 1, 'repair: network = HTTP bytes');
  near(sn.session.bytes.write, 40 * MiB, 1, 'repair: written (assemble reached its total)');
}

// --- bursts from several worker threads, delivered out of order ---
{
  // Bytes that land in a bucket only as same-millisecond points still count
  // in a window ending exactly at that millisecond.
  const s = new RateSeries(10);
  s.advance(T0);
  s.add('read', 1000, T0 + 500, T0 + 500);
  near(s.rate('read', T0 + 500, 1000), 1000, 1e-9, 'point bytes at "now" are in the window');
  near(s.rate('read', T0 + 499, 1000), 0, 0, 'point bytes after the window end are not');

  // 4 verify workers share one atomic counter (fetch_add, then report). Each
  // 100 ms they all report within a few ms, and a worker that read the
  // counter first is sometimes delivered last.
  const mb = new Meter();
  mb.start('b', T0);
  const size = 4096 * MiB;
  mb.progress('b', 'verify', 0, size, T0, null);
  let counter = 0;
  let shown = 0;
  let backwards = false;
  let tt = T0;
  for (let i = 1; i <= 80; i++) {
    const base = T0 + i * 100;
    const values: number[] = [];
    for (let w = 0; w < 4; w++) values.push((counter += 2.5 * MiB)); // 100 MiB/s in total
    // Delivery order: 2nd, 1st, 4th, 3rd; two of them in the same millisecond.
    const order = [values[1], values[0], values[3], values[2]];
    const times = [base, base, base + 1, base + 3];
    order.forEach((v, k) => {
      const next = mb.progress('b', 'verify', v, size, times[k], null);
      if (next < shown) backwards = true;
      shown = next;
    });
    tt = base + 3;
    if (i % 10 === 0) mb.tick(tt);
  }
  const sn = mb.snapshot(tt);
  ok(!backwards, 'burst: displayed value never goes backwards');
  near(sn.session.bytes.read, counter, 1, 'burst: every byte counted exactly once');
  near(shown, counter, 0, 'burst: display ends at the highest counter value');
  near(sn.rates.read, 100 * MiB, 2 * MiB, 'burst: read rate');
  near(sn.streams.b.rate, 100 * MiB, 2 * MiB, 'burst: op rate');
  ok(Number.isFinite(sn.streams.b.eta ?? NaN), 'burst: finite time left');
  const full = sn.history.filter((b) => b.t >= T0 + 1000 && b.t + STEP <= T0 + 8000);
  ok(full.length === 7 && full.every((b) => Math.abs(b.read - 100 * MiB) < 3 * MiB), 'burst: buckets are even, no spikes');

  // HTTP chunks finishing together: increasing values in the same millisecond add up exactly.
  const mh = new Meter();
  mh.start('n', T0);
  mh.progress('n', 'download', 0, 100 * MiB, T0, 'http');
  for (let k = 1; k <= 8; k++) mh.progress('n', 'download', k * MiB, 100 * MiB, T0 + 1000, 'http');
  near(mh.snapshot(T0 + 1000).session.bytes.net, 8 * MiB, 0, 'same-ms HTTP burst counted once');
  near(mh.snapshot(T0 + 1000).streams.n.rate, 8 * MiB, 1, 'same-ms HTTP burst rate');
}

console.log(`throughput: ${checks} checks passed`);

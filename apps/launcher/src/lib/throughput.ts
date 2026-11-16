// Transfer-rate bookkeeping for the Downloads view.
//
// Plain TypeScript (no Svelte, no DOM) so the math can be reasoned about and
// exercised on its own: `deno run scripts/check-throughput.ts`.
//
// Inputs are cumulative byte counters, not rates:
// * progress events `{ phase, source, done, total }` per operation (~10/s
//   while busy). A download reports one counter per source: first "peers"
//   (BitTorrent) with the torrent's total, then "http" with its own total
//   for whatever the swarm did not deliver;
// * the polled per-game `uploaded_bytes` of seeding (every few seconds).
// Deltas between consecutive observations are spread evenly over the time
// between them and binned into one-second buckets. Rates are moving averages
// over those buckets ending "now", so they decay to zero by themselves when
// events stop arriving instead of freezing at the last value.

/**
 * What a byte counter measures. `net` is everything downloaded; `peers` is
 * the part of `net` that came from other players (so server bytes are
 * `net - peers`). `read` is reading and checking installed files (verify,
 * harvest), `write` is writing game files, `up` is upload to peers.
 */
export type Channel = 'net' | 'peers' | 'write' | 'read' | 'up';

export const CHANNELS: readonly Channel[] = ['net', 'peers', 'write', 'read', 'up'];

/** Where the bytes of a download come from (null outside the download phase). */
export type Source = 'http' | 'peers';

/** Bytes moved per channel during one bucket (`t` = bucket start, ms epoch). */
export interface Bucket {
  t: number;
  net: number;
  /** Subset of `net`. */
  peers: number;
  write: number;
  read: number;
  up: number;
  /**
   * The part of the bucket the bytes actually fell in (Infinity / -Infinity
   * while empty). Without it a bucket that a phase starts or ends inside
   * would be treated as evenly filled, skewing rates by up to a second's worth.
   */
  lo: number;
  hi: number;
}

export type Rates = Record<Channel, number>;

/** Bucket width; with 1 s buckets, bytes per bucket = bytes per second. */
export const STEP = 1000;
/** Seconds of history kept for the graph. */
export const HISTORY_SECONDS = 300;
/** Window of the "current speed" moving average. */
export const RATE_WINDOW = 4000;
/** Longer window for time-remaining estimates, which should not jump around. */
export const ETA_WINDOW = 15000;
/** Upload counters are polled every ~3 s, so average over two polls. */
export const UP_WINDOW = 6000;
/** A progress delta is never spread over more than this (stale baselines). */
export const MAX_SPREAD = 5000;
/** An operation must have been in its phase this long before an ETA is shown. */
export const ETA_MIN_ELAPSED = 3000;
/** No network bytes for this long during a download = stalled. */
export const STALL_AFTER = 5000;
/** Time-remaining estimates beyond this are not meaningful. */
const ETA_MAX_SECONDS = 7 * 24 * 3600;
/** Wall-clock gaps longer than this (sleep, throttled timers) are not "download time". */
const MAX_ACCRUE = 60000;

/** Which channel the byte counter of an engine phase measures. */
export function channelOf(phase: string): Channel | null {
  switch (phase) {
    case 'download':
      return 'net';
    case 'assemble':
      return 'write';
    case 'harvest':
    case 'verify':
      return 'read';
    default:
      // 'starting', 'done': no bytes move (the final `done` event repeats the full size).
      return null;
  }
}

function zeroRates(): Rates {
  return { net: 0, peers: 0, write: 0, read: 0, up: 0 };
}

function emptyBucket(t: number): Bucket {
  return { t, net: 0, peers: 0, write: 0, read: 0, up: 0, lo: Infinity, hi: -Infinity };
}

/** A ring of one-second buckets; the last bucket is the one still filling. */
export class RateSeries {
  readonly buckets: Bucket[] = [];
  /** Bytes ever added, including those spread past the start of the ring. */
  readonly totals: Rates = zeroRates();
  /** Latest time seen; never moves backwards. */
  now = 0;

  constructor(readonly capacity: number) {}

  /** Move "now" forward, closing buckets and zero-filling any gap. */
  advance(now: number): void {
    if (now < this.now) return; // clock stepped back: keep the latest time
    this.now = now;
    const cur = Math.floor(now / STEP) * STEP;
    const last = this.buckets[this.buckets.length - 1];
    if (last && cur <= last.t) return;
    let t = last ? last.t + STEP : cur;
    const oldest = cur - (this.capacity - 1) * STEP;
    if (t < oldest) {
      // Gap longer than the whole ring: nothing in it survives.
      this.buckets.length = 0;
      t = oldest;
    }
    for (; t <= cur; t += STEP) this.buckets.push(emptyBucket(t));
    if (this.buckets.length > this.capacity) this.buckets.splice(0, this.buckets.length - this.capacity);
  }

  /** Record `bytes` moved evenly during [from, to]. `to` is normally "now". */
  add(ch: Channel, bytes: number, from: number, to: number): void {
    if (!(bytes > 0)) return;
    this.advance(to);
    to = Math.min(to, this.now);
    from = Math.min(from, to);
    this.totals[ch] += bytes;
    // A young ring may not reach back to `from` yet: grow it backwards.
    const floor = Math.max(Math.floor(from / STEP) * STEP, this.buckets[this.buckets.length - 1].t - (this.capacity - 1) * STEP);
    const head: Bucket[] = [];
    for (let t = floor; t < this.buckets[0].t; t += STEP) head.push(emptyBucket(t));
    if (head.length) this.buckets.unshift(...head);
    const span = to - from;
    if (span < 1) {
      const b = this.bucketAt(to);
      if (!b) return;
      b[ch] += bytes;
      b.lo = Math.min(b.lo, to);
      b.hi = Math.max(b.hi, to);
      return;
    }
    for (let i = this.buckets.length - 1; i >= 0; i--) {
      const b = this.buckets[i];
      if (b.t + STEP <= from) break;
      const lo = Math.max(from, b.t);
      const hi = Math.min(to, b.t + STEP);
      if (hi <= lo) continue;
      b[ch] += (bytes * (hi - lo)) / span;
      b.lo = Math.min(b.lo, lo);
      b.hi = Math.max(b.hi, hi);
    }
  }

  private bucketAt(t: number): Bucket | undefined {
    for (let i = this.buckets.length - 1; i >= 0; i--) if (this.buckets[i].t <= t) return this.buckets[i];
    return undefined;
  }

  /** Bytes in (start, end], taking each bucket's bytes as spread evenly over its [lo, hi]. */
  sum(ch: Channel, start: number, end: number): number {
    let total = 0;
    for (let i = this.buckets.length - 1; i >= 0; i--) {
      const b = this.buckets[i];
      if (b.t + STEP <= start) break;
      if (!b[ch]) continue;
      if (b.hi - b.lo < 1) {
        // Only same-millisecond events landed here (a burst from several
        // workers): a point, counted if it lies in the range. Treating it as
        // [lo, lo + 1] would drop it from a window that ends exactly at lo.
        if (b.lo > start && b.lo <= end) total += b[ch];
        continue;
      }
      const overlap = Math.min(end, b.hi) - Math.max(start, b.lo);
      if (overlap > 0) total += (b[ch] * overlap) / (b.hi - b.lo);
    }
    return total;
  }

  /** Average bytes per second over the `window` ms ending at `end`. */
  rate(ch: Channel, end: number, window: number): number {
    if (window <= 0) return 0;
    return this.sum(ch, end - window, end) / (window / 1000);
  }

  /** Copies of the closed buckets (everything but the one still filling). */
  closed(): Bucket[] {
    return this.buckets.slice(0, -1).map((b) => ({ ...b }));
  }
}

interface Stream {
  phase: string;
  /** Download source of the current counter; null outside the download phase. */
  source: Source | null;
  /** Start of the current phase (or of the current counter after a restart). */
  phaseStart: number;
  done: number;
  total: number;
  /** Bytes counted since `phaseStart`. */
  phaseBytes: number;
  /** Time of the last observation. */
  t: number;
  /** Last time the counter of a byte-moving phase went up. */
  lastGain: number;
  /** The current phase has moved at least one byte. */
  gained: boolean;
  series: RateSeries;
  /** Highest smoothed network rate seen, bytes/s. */
  peak: number;
  netBytes: number;
  /** Part of `netBytes` that came from peers. */
  peerBytes: number;
  /** Wall time spent in the download phase. */
  netMs: number;
}

export interface StreamStats {
  phase: string;
  source: Source | null;
  done: number;
  total: number;
  /** Smoothed rate of what the current phase does (network, disk write or disk read). */
  rate: number;
  /** Smoothed network rate; zero outside the download phase. */
  net: number;
  /** Bytes downloaded by this operation so far, all sources. */
  netBytes: number;
  /** Part of `netBytes` that came from peers. */
  peerBytes: number;
  peak: number;
  /** Network bytes / time spent downloading; null until there is enough data. */
  average: number | null;
  /** Seconds left in the current phase; null when not meaningful. */
  eta: number | null;
  /** Downloading, but no bytes for a while. */
  stalled: boolean;
}

export interface Totals {
  /** Bytes per channel since the meter was created. */
  bytes: Rates;
  /** Highest smoothed combined download rate. */
  peak: number;
  /** Network bytes / wall time with at least one download running; null if none yet. */
  average: number | null;
}

export interface TrafficSnapshot {
  now: number;
  /** Closed one-second buckets, oldest first, ending at the last full second. */
  history: Bucket[];
  /** Upload data is complete up to here (it arrives in polls); later buckets may still fill. */
  upUntil: number;
  /** Current smoothed rates, bytes/s. */
  rates: Rates;
  streams: Record<string, StreamStats>;
  session: Totals;
  /** The current batch: since an operation started while none was running. Null when idle. */
  batch: BatchTotals | null;
}

export interface BatchTotals {
  /** Network bytes of the batch. */
  net: number;
  /** Part of `net` that came from peers. */
  peers: number;
  peak: number;
  average: number | null;
}

interface Batch {
  peak: number;
  netBytes: number;
  peerBytes: number;
  netMs: number;
}

export function emptySnapshot(now = 0): TrafficSnapshot {
  return {
    now,
    history: [],
    upUntil: now,
    rates: zeroRates(),
    streams: {},
    session: { bytes: zeroRates(), peak: 0, average: null },
    batch: null,
  };
}

function average(bytes: number, ms: number): number | null {
  return ms >= 2000 ? bytes / (ms / 1000) : null;
}

/**
 * Turns progress events and upload counters into rates, peaks, averages and
 * a short history. Feed it `start` / `progress` / `end` for operations,
 * `counter` for upload counters, call `tick` about once a second while
 * anything is happening, and read `snapshot`.
 */
export class Meter {
  readonly global = new RateSeries(HISTORY_SECONDS + 1);
  private streams = new Map<string, Stream>();
  private counters = new Map<string, { value: number; t: number }>();
  private lastUp = 0;
  private lastActivity = -Infinity;
  private lastAccrue = 0;
  private session = { peak: 0, netMs: 0 };
  private batch: Batch | null = null;

  /** An operation began (before its first progress event). */
  start(key: string, now: number): void {
    this.accrue(now);
    if (!this.batch && this.streams.size === 0) this.batch = { peak: 0, netBytes: 0, peerBytes: 0, netMs: 0 };
    this.streams.set(key, this.newStream('starting', null, 0, 0, now));
  }

  /**
   * A progress event of operation `key`. `done` counts bytes of the current
   * phase (and, while downloading, of the current source). Returns the
   * counter value to display: an out-of-order local-disk event is ignored,
   * so this can be higher than `done`.
   */
  progress(key: string, phase: string, done: number, total: number, now: number, source: Source | null = null): number {
    const s = this.streams.get(key);
    if (!s) {
      this.accrue(now);
      // Not announced with `start`: take this event as the baseline.
      if (!this.batch && this.streams.size === 0) this.batch = { peak: 0, netBytes: 0, peerBytes: 0, netMs: 0 };
      this.streams.set(key, this.newStream(phase, source, done, total, now));
      return done;
    }
    const ch = channelOf(phase);
    const from = Math.max(s.t, now - MAX_SPREAD);
    let delta: number;
    if (phase !== s.phase || source !== s.source) {
      // A new counter: every engine phase counts up from zero, and so does
      // each download source (peers first, then HTTP for the rest). The
      // shell always delivers the first event after such a change.
      if (phase !== s.phase) s.gained = false;
      s.phase = phase;
      s.source = source;
      // With nothing counted yet the new counter starts now; otherwise its
      // bytes moved since the previous event.
      s.phaseStart = done > 0 ? from : now;
      s.phaseBytes = 0;
      delta = done;
    } else if (done >= s.done) {
      delta = done - s.done;
    } else if (ch === 'net') {
      // A download counter went backwards: bytes of a failed HTTP attempt
      // (or a BitTorrent piece that failed its hash) were un-counted.
      // Rebase; the bytes did cross the network, but they are fetched again
      // and will be counted then.
      delta = 0;
    } else {
      // Local-disk counters never un-count. A lower value is a report from a
      // worker thread that read the shared counter before another one but
      // was delivered after it; taking it as the new baseline would count
      // the gap twice. Keep the higher value and ignore the event.
      if (total !== s.total) s.total = total;
      return s.done;
    }
    // The time since the previous event belongs to the phase this event
    // reports (its bytes are spread over it), so accrue after the switch.
    this.accrue(now);
    s.done = done;
    s.total = total;
    s.t = now;
    if (ch && delta > 0) {
      s.series.add(ch, delta, from, now);
      this.global.add(ch, delta, from, now);
      s.phaseBytes += delta;
      s.lastGain = now;
      s.gained = true;
      this.lastActivity = now;
      if (ch === 'net') {
        s.netBytes += delta;
        if (this.batch) this.batch.netBytes += delta;
        if (source === 'peers') {
          s.series.add('peers', delta, from, now);
          this.global.add('peers', delta, from, now);
          s.peerBytes += delta;
          if (this.batch) this.batch.peerBytes += delta;
        }
      }
    }
    return done;
  }

  /** The operation finished (successfully or not). */
  end(key: string, now: number): void {
    this.accrue(now);
    this.streams.delete(key);
    if (this.streams.size === 0) this.batch = null;
  }

  /** A cumulative counter (seeding upload) was polled. */
  counter(key: string, value: number, now: number): void {
    const prev = this.counters.get(key);
    this.counters.set(key, { value, t: now });
    this.lastUp = now;
    if (!prev || value <= prev.value) return; // first sight, or the seed restarted
    const delta = value - prev.value;
    this.global.add('up', delta, Math.max(prev.t, now - HISTORY_SECONDS * STEP), now);
    this.lastActivity = now;
  }

  /** Forget counters that are no longer reported (seed stopped). */
  retainCounters(keys: Iterable<string>, now: number): void {
    const keep = new Set(keys);
    for (const k of [...this.counters.keys()]) if (!keep.has(k)) this.counters.delete(k);
    this.lastUp = now;
  }

  /** Close buckets and update peaks; call about once a second while active. */
  tick(now: number): void {
    this.accrue(now);
    this.global.advance(now);
    let net = 0;
    for (const s of this.streams.values()) {
      s.series.advance(now);
      if (channelOf(s.phase) !== 'net') continue;
      const r = this.channelRate(s, 'net', now, RATE_WINDOW);
      if (r > s.peak) s.peak = r;
      net += r;
    }
    if (this.batch && net > this.batch.peak) this.batch.peak = net;
    if (net > this.session.peak) this.session.peak = net;
  }

  /** Nothing running and every rate has decayed: the caller may stop ticking. */
  idle(now: number): boolean {
    return this.streams.size === 0 && now - this.lastActivity > Math.max(RATE_WINDOW, UP_WINDOW) + STEP;
  }

  snapshot(now: number): TrafficSnapshot {
    this.global.advance(now);
    const rates = zeroRates();
    const streams: Record<string, StreamStats> = {};
    for (const [key, s] of this.streams) {
      s.series.advance(now);
      const st = this.streamStats(s, now);
      streams[key] = st;
      // Only the current phase counts: once a download is done and files are
      // being written, its download speed is zero, not a fading average.
      const ch = channelOf(s.phase);
      if (ch) rates[ch] += st.rate;
      if (ch === 'net' && s.source === 'peers') rates.peers += st.rate;
    }
    // Upload data lags by up to one poll; measure up to the last poll unless
    // polling stopped, in which case the rate decays from now.
    const upEnd = now - this.lastUp < 2 * UP_WINDOW ? this.lastUp : now;
    rates.up = this.global.rate('up', upEnd, UP_WINDOW);
    return {
      now,
      history: this.global.closed(),
      upUntil: Math.min(now, this.lastUp || now),
      rates,
      streams,
      session: {
        bytes: { ...this.global.totals },
        peak: this.session.peak,
        average: average(this.global.totals.net, this.session.netMs),
      },
      batch: this.batch
        ? {
            net: this.batch.netBytes,
            peers: this.batch.peerBytes,
            peak: this.batch.peak,
            average: average(this.batch.netBytes, this.batch.netMs),
          }
        : null,
    };
  }

  // ---- internals ----

  private newStream(phase: string, source: Source | null, done: number, total: number, now: number): Stream {
    return {
      phase,
      source,
      phaseStart: now,
      done,
      total,
      phaseBytes: 0,
      t: now,
      lastGain: now,
      gained: false,
      series: new RateSeries(Math.ceil(ETA_WINDOW / STEP) + 5),
      peak: 0,
      netBytes: 0,
      peerBytes: 0,
      netMs: 0,
    };
  }

  /**
   * Smoothed rate of one channel of a stream. A phase younger than the
   * window is averaged over its own age (from its exact byte count), so a
   * fresh download neither ramps up slowly nor inherits the previous counter.
   */
  private channelRate(s: Stream, ch: Channel, now: number, window: number): number {
    if (channelOf(s.phase) === ch) {
      const age = now - s.phaseStart;
      if (age < window) return s.phaseBytes / (Math.max(STEP, age) / 1000);
    }
    return s.series.rate(ch, now, window);
  }

  private streamStats(s: Stream, now: number): StreamStats {
    const ch = channelOf(s.phase);
    const rate = ch ? this.channelRate(s, ch, now, RATE_WINDOW) : 0;
    const stalled = ch === 'net' && s.gained && now - s.lastGain > STALL_AFTER;
    let eta: number | null = null;
    if (ch && rate > 0 && !stalled && s.total > 0 && s.done < s.total && now - s.phaseStart >= ETA_MIN_ELAPSED) {
      const r = this.channelRate(s, ch, now, ETA_WINDOW);
      if (r > 0) {
        const secs = (s.total - s.done) / r;
        if (secs <= ETA_MAX_SECONDS) eta = secs;
      }
    }
    return {
      phase: s.phase,
      source: s.source,
      done: s.done,
      total: s.total,
      rate,
      net: ch === 'net' ? rate : 0,
      netBytes: s.netBytes,
      peerBytes: s.peerBytes,
      peak: s.peak,
      average: average(s.netBytes, s.netMs),
      eta,
      stalled,
    };
  }

  /** Add wall time to "time spent downloading" for streams in the download phase. */
  private accrue(now: number): void {
    const dt = Math.min(MAX_ACCRUE, Math.max(0, now - this.lastAccrue));
    this.lastAccrue = Math.max(this.lastAccrue, now);
    if (!dt) return;
    let downloading = false;
    for (const s of this.streams.values()) {
      if (channelOf(s.phase) !== 'net') continue;
      s.netMs += dt;
      downloading = true;
    }
    if (!downloading) return;
    this.session.netMs += dt;
    if (this.batch) this.batch.netMs += dt;
  }
}

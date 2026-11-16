// In-memory mock backend used when the UI runs in a plain browser (vite dev,
// screenshots, UI work without a Rust toolchain). All games are fictional.

import type {
  BackendError,
  DiscoverEntry,
  DiscoverResult,
  DiskSpace,
  ExitedEvent,
  InstallOptions,
  InstallReport,
  LibraryEntry,
  ProgressEvent,
  SeedFailure,
  Source,
  SeedingInfo,
  SeedStatus,
  Settings,
  SettingsPatch,
  UpdateInfo,
  VerifyReport,
} from './types';

type Listener<T> = (payload: T) => void;

const KEY_A = 'ed25519:Q2xvdWRiZXJyeS1kZXYta2V5LWV4YW1wbGUtMDAwMDAwMQ';
const KEY_B = 'ed25519:TW9zc3dvcmtzLWNvbGxlY3RpdmUta2V5LWV4YW1wbGUwMg';
const KEY_C = 'ed25519:VGlkZXdhbGtlci1zdHVkaW8ta2V5LWV4YW1wbGUtMDAwMw';
const KEY_D = 'ed25519:TGFudGVybi1ob2xsb3ctdGVhbS1rZXktZXhhbXBsZTAwNA';
const KEY_ROTATED = 'ed25519:Um90YXRlZC1vci1pbXBvc3Rlci1rZXktZXhhbXBsZTAwNQ';

const ASSETS = 'https://play.example.invalid/chunks/';
const hex = (seed: string) =>
  Array.from({ length: 64 }, (_, i) => ((seed.charCodeAt(i % seed.length) * (i + 7)) % 16).toString(16)).join('');

interface MockRelease {
  version: string;
  size: number;
  changelog: string;
}

interface MockGame {
  entry: LibraryEntry;
  latest: MockRelease;
  server: string;
  serverName: string;
}

function game(
  slug: string,
  title: string,
  summary: string,
  description: string,
  key: string,
  server: string,
  serverName: string,
  latest: MockRelease,
  installed: string | null,
): MockGame {
  return {
    server,
    serverName,
    latest,
    entry: {
      game_id: hex(slug).slice(0, 26).replace(/[0-9]/g, (d) => 'abcdefghij'[Number(d)]),
      slug,
      title,
      summary,
      description_md: description,
      sources: [server],
      pinned_key: key,
      channel: 'stable',
      channels: { stable: latest.version, beta: bump(latest.version) },
      icon: hex(slug + 'icon'),
      banner: hex(slug + 'banner'),
      screenshots: [hex(slug + 's1'), hex(slug + 's2')],
      asset_base: ASSETS,
      installed: installed
        ? {
            version: installed,
            platform: 'linux-x86_64',
            dir: `/home/player/Games/${slug}`,
            installed_at: '2026-10-02T18:21:07Z',
            // An older installed version is a little smaller than the latest.
            size: installed === latest.version ? latest.size : Math.round(latest.size * 0.94),
          }
        : null,
    },
  };
}

function bump(v: string): string {
  const [maj, min] = v.split('.').map(Number);
  return `${maj}.${min + 1}.0-beta.1`;
}

const GiB = 1024 ** 3;
const MiB = 1024 ** 2;

const catalogGames: MockGame[] = [
  game(
    'starfall-courier',
    'Starfall Courier',
    'Deliver parcels across a collapsing star system before the lights go out.',
    `# Starfall Courier

A physics-driven delivery game set in a dying solar system. Pilot a battered
cargo skiff, juggle **gravity wells** and keep your packages intact.

## Features

- 40 handcrafted routes and a procedural *endless* mode
- Ship upgrades: thrusters, tethers, cryo-holds
- Local co-op for two pilots

Made with love by the Cloudberry crew. Source and modding docs at
[the project page](https://example.org/starfall).`,
    KEY_A,
    'https://play.example.invalid',
    'Example Play',
    {
      version: '1.4.0',
      size: Math.round(1.7 * GiB),
      changelog: `## 1.4.0

- New **Eclipse** sector with 6 routes
- Controller rumble support
- Fixed a crash when docking at \`Station 9\` with a full hold`,
    },
    '1.3.2',
  ),
  game(
    'moss-and-mortar',
    'Moss & Mortar',
    'A cosy city-builder where buildings slowly turn back into forest.',
    `Build a village, then watch nature take it back.

1. Lay out streets and gardens
2. Balance growth against decay
3. Retire gracefully

> "The most relaxing thing on my hard drive." — a playtester`,
    KEY_B,
    'https://play.example.invalid',
    'Example Play',
    { version: '0.9.1', size: 640 * MiB, changelog: '## 0.9.1\n\n- Autumn palette\n- Save files are now 60% smaller' },
    '0.9.1',
  ),
  game(
    'tidewalker',
    'Tidewalker',
    'Explore a drowned archipelago between low and high tide.',
    'Every six minutes the sea rises. Map the islands, find the lighthouse keepers, and get back before the water does.',
    KEY_C,
    'https://indie.example.invalid',
    'Indie Mirror',
    { version: '2.0.0', size: Math.round(3.2 * GiB), changelog: '## 2.0.0\n\n- Full release!\n- Photo mode\n- Steam Deck controls' },
    null,
  ),
  game(
    'lantern-hollow',
    'Lantern Hollow',
    'A short narrative mystery told by candlelight.',
    'A two-hour story game about a village that forgot how to sleep.',
    KEY_D,
    'https://indie.example.invalid',
    'Indie Mirror',
    { version: '1.0.3', size: 410 * MiB, changelog: '## 1.0.3\n\n- Typo fixes\n- Better font rendering on HiDPI screens' },
    null,
  ),
];

function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

function fail(message: string, extra: Partial<BackendError> = {}): never {
  throw { kind: 'error', message, ...extra } satisfies BackendError;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * A plausible, wobbly transfer rate (bytes/s) at `t` ms into a transfer:
 * two slow waves, jitter, and now and then a few seconds of congestion.
 */
function wobble(base: number, t: number, seed: number): number {
  const waves = 1 + 0.22 * Math.sin(t / 2300 + seed) + 0.12 * Math.sin(t / 610 + seed * 3);
  const jitter = 0.9 + Math.random() * 0.2;
  const phase = (t + seed * 7919) % 26000;
  const congestion = phase > 17000 && phase < 19500 ? 0.18 : 1;
  return Math.max(0, base * waves * jitter * congestion);
}

const TiB = 1024 ** 4;

export class MockBackend {
  private settings: Settings = {
    install_root: '/home/player/Games',
    bandwidth_limit: null,
    concurrency: 8,
    keep_chunks: false,
    transport: 'torrent-direct',
    seed: true,
    upload_limit: 2 * MiB,
    wine: null,
    listen_port: 0,
    torrent_stall_secs: 20,
  };
  private library = new Map<string, LibraryEntry>();
  private catalogList = ['https://play.example.invalid', 'https://indie.example.invalid'];
  private running = new Set<string>();
  private progressListeners = new Set<Listener<ProgressEvent>>();
  private exitedListeners = new Set<Listener<ExitedEvent>>();
  private seedFailureListeners = new Set<Listener<SeedFailure[]>>();
  /** game_id → seed state; `at` is when `uploaded` was last advanced. */
  private seeds = new Map<string, { since: number; at: number; uploaded: number; total: number }>();
  private engineStarted = false;
  /** Bytes the mock has "written" to the main disk (shrinks its free space). */
  private written = 0;
  private startup: { listening: boolean; failures: SeedFailure[] } = { listening: false, failures: [] };

  constructor() {
    for (const g of catalogGames.slice(0, 3)) this.library.set(g.entry.game_id, clone(g.entry));
    // Like the app: resume seeding in the background at startup.
    void this.startSeeding().then((failures) => {
      if (!failures.length) return;
      if (this.startup.listening) for (const l of this.seedFailureListeners) l(failures);
      else this.startup.failures.push(...failures);
    });
  }

  onSeedFailures(cb: Listener<SeedFailure[]>) {
    this.seedFailureListeners.add(cb);
    return () => this.seedFailureListeners.delete(cb);
  }

  onProgress(cb: Listener<ProgressEvent>) {
    this.progressListeners.add(cb);
    return () => this.progressListeners.delete(cb);
  }
  onExited(cb: Listener<ExitedEvent>) {
    this.exitedListeners.add(cb);
    return () => this.exitedListeners.delete(cb);
  }

  private emit(ev: ProgressEvent) {
    for (const l of this.progressListeners) l(ev);
  }

  private resolve(selector: string): LibraryEntry {
    for (const e of this.library.values()) {
      if (e.game_id === selector || e.slug === selector || e.sources.some((s) => `${s}/games/${e.slug}` === selector))
        return e;
    }
    fail(`no game matching "${selector}" in the library`);
  }

  private catalogFor(entry: LibraryEntry): MockGame {
    return catalogGames.find((g) => g.entry.slug === entry.slug)!;
  }

  async getSettings(): Promise<Settings> {
    return clone(this.settings);
  }
  async updateSettings(patch: SettingsPatch): Promise<Settings> {
    const wasSeeding = this.settings.seed;
    // Only the given fields change, like the real backend.
    this.settings = { ...this.settings, ...clone(patch) };
    if (wasSeeding && !this.settings.seed) this.seeds.clear();
    return clone(this.settings);
  }
  async listGames(): Promise<LibraryEntry[]> {
    await sleep(60);
    return [...this.library.values()].map(clone).sort((a, b) => a.title.localeCompare(b.title));
  }
  async findGame(selector: string): Promise<LibraryEntry> {
    return clone(this.resolve(selector));
  }

  async addGame(url: string, acceptKeyChange: boolean): Promise<LibraryEntry> {
    await sleep(400);
    const m = /^(?:https?|gamedist(?:\+http)?):\/\/([^/]+)\/games\/([a-z0-9-]+)\/?$/.exec(url.trim());
    if (!m) fail(`not a game URL: ${url} (expected https://host/games/<slug> or gamedist://host/games/<slug>)`);
    const slug = m[2];
    if (slug === 'starfall-courier' && m[1].startsWith('evil') && !acceptKeyChange) {
      // Demo of the key-change flow: `gamedist://evil.example/games/starfall-courier`.
      throw {
        kind: 'key_changed',
        message: `developer key for starfall-courier changed at https://${m[1]}`,
        pinned: KEY_A,
        presented: KEY_ROTATED,
        slug,
        source_url: `https://${m[1]}/games/${slug}`,
      } satisfies BackendError;
    }
    const g = catalogGames.find((c) => c.entry.slug === slug);
    if (!g) fail(`https://${m[1]}/games/${slug}: server returned 404: Not Found`);
    const existing = this.library.get(g.entry.game_id);
    if (existing) {
      const src = `https://${m[1]}`;
      if (!existing.sources.includes(src)) existing.sources.push(src);
      return clone(existing);
    }
    const entry = clone(g.entry);
    entry.installed = null;
    this.library.set(entry.game_id, entry);
    return clone(entry);
  }

  async checkUpdate(selector: string): Promise<UpdateInfo> {
    await sleep(150);
    const e = this.resolve(selector);
    const g = this.catalogFor(e);
    const installed = e.installed?.version ?? null;
    const update = installed !== g.latest.version;
    return {
      game_id: e.game_id,
      installed,
      latest: g.latest.version,
      update_available: installed !== null && update,
      download_bytes: !update ? 0 : installed ? Math.round(g.latest.size * 0.12) : g.latest.size,
      total_bytes: g.latest.size,
      changelog_md: g.latest.changelog,
    };
  }

  /**
   * Emit progress for one phase at a varying rate, like the throttled engine
   * events (~10/s). `source` is set for downloads only. Stops at `stopAt`
   * bytes (a torrent swarm that ran dry).
   *
   * `workers` > 1 imitates a disk phase whose workers share one counter
   * (verify): now and then a report that read the counter earlier is
   * delivered right after a later one. `failedAttempt` un-counts a few MiB
   * once, halfway (an HTTP chunk that failed and is fetched again).
   */
  private async runPhase(
    game_id: string,
    op: ProgressEvent['op'],
    phase: ProgressEvent['phase'],
    total: number,
    rate: (t: number) => number,
    opts: { source?: Source; stopAt?: number; workers?: number; failedAttempt?: boolean } = {},
  ): Promise<number> {
    const tick = 100;
    const source = opts.source ?? null;
    const stopAt = opts.stopAt ?? total;
    let done = 0;
    let t = 0;
    let failed = !opts.failedAttempt;
    this.emit({ game_id, op, phase, source, done, total });
    while (done < stopAt) {
      await sleep(tick);
      t += tick;
      const prev = done;
      done = Math.min(stopAt, done + (rate(t) * tick) / 1000);
      this.emit({ game_id, op, phase, source, done: Math.round(done), total });
      if ((opts.workers ?? 1) > 1 && done < stopAt && Math.random() < 0.2) {
        const stale = Math.round(prev + (done - prev) * Math.random());
        this.emit({ game_id, op, phase, source, done: stale, total });
      }
      if (!failed && done >= stopAt / 2 && done > 6 * MiB) {
        failed = true;
        done -= 3 * MiB;
        this.emit({ game_id, op, phase, source, done: Math.round(done), total });
      }
    }
    return Math.round(done);
  }

  /** Network speed in bytes/s, honouring the bandwidth limit like the engine. */
  private netRate(base: number, seed: number) {
    const limit = this.settings.bandwidth_limit;
    return (t: number) => {
      const r = wobble(base, t, seed);
      return limit ? Math.min(r, limit * (0.92 + Math.random() * 0.08)) : r;
    };
  }

  async install(selector: string, opts: InstallOptions): Promise<InstallReport> {
    const e = this.resolve(selector);
    const g = this.catalogFor(e);
    const previous = e.installed?.version ?? null;
    const previousSize = e.installed?.size ?? 0;
    // Chunks the new version needs that are not in the installed one.
    const missing = previous ? Math.round(g.latest.size * 0.12) : g.latest.size;
    // Bytes of the files that change (written in full during assemble).
    const writeBytes = previous ? Math.round(g.latest.size * 0.3) : g.latest.size;
    const seed = g.entry.slug.length;
    const id = e.game_id;
    if (previous) {
      // Read the reusable chunks out of the installed files (bytes read).
      const reusable = writeBytes - missing;
      await this.runPhase(id, 'install', 'harvest', reusable, (t) => wobble(380 * MiB, t, seed + 3));
    }
    // Torrent first: the swarm fetches whole changed files (the torrent's
    // totals), peers connect over a few seconds. With "torrent + direct" the
    // swarm runs dry at ~70 % and HTTP fetches the rest with its own,
    // smaller total; with "torrent" only, the swarm delivers everything.
    let fromPeers = 0;
    let torrentBytes = 0;
    const transport = this.settings.transport;
    if (transport !== 'direct') {
      const peers = this.netRate(26 * MiB, seed);
      const share = transport === 'torrent' ? 1 : 0.7;
      fromPeers = await this.runPhase(
        id,
        'install',
        'download',
        writeBytes,
        (t) => peers(t) * Math.min(1, 0.15 + t / 6000),
        { source: 'peers', stopAt: Math.round(writeBytes * share) },
      );
      torrentBytes = Math.round(missing * share);
      // Stall timeout, then the engine hashes what arrived (no events).
      await sleep(share < 1 ? 1800 : 600);
    }
    const rest = missing - torrentBytes;
    if (rest > 0) await this.runPhase(id, 'install', 'download', rest, this.netRate(34 * MiB, seed + 1), { source: 'http', failedAttempt: true });
    await this.runPhase(id, 'install', 'assemble', writeBytes, (t) => wobble(210 * MiB, t, seed + 2));
    this.written += g.latest.size - previousSize;
    this.emit({ game_id: id, op: 'install', phase: 'done', source: null, done: g.latest.size, total: g.latest.size });
    e.installed = {
      version: g.latest.version,
      platform: 'linux-x86_64',
      dir: opts.dir || e.installed?.dir || `${this.settings.install_root}/${e.slug}`,
      installed_at: new Date().toISOString(),
      size: g.latest.size,
    };
    if (fromPeers) this.engineStarted = true;
    this.seeds.delete(e.game_id);
    const seeding = this.settings.seed;
    if (seeding) this.startSeed(e.game_id, g.latest.size);
    return {
      game_id: e.game_id,
      version: g.latest.version,
      previous_version: previous,
      platform: 'linux-x86_64',
      dir: e.installed.dir,
      downloaded_chunks: Math.ceil(rest / (2 * MiB)),
      downloaded_bytes: rest,
      harvested_bytes: previous ? writeBytes - missing : 0,
      torrent_bytes: torrentBytes,
      files_written: 37,
      files_deleted: previous ? 2 : 0,
      up_to_date: previous === g.latest.version,
      seeding,
    };
  }

  /**
   * Checks the installed files (bytes, from several workers). Moss & Mortar
   * always "finds" two damaged files, to show a repair: harvest of the intact
   * parts, an HTTP download of the rest, then writing the repaired files.
   * Like the engine, no `done` event follows.
   */
  async verify(selector: string, repair: boolean): Promise<VerifyReport> {
    const e = this.resolve(selector);
    if (!e.installed) fail(`${e.slug} is not installed`);
    const id = e.game_id;
    const size = e.installed.size || this.catalogFor(e).latest.size;
    await this.runPhase(id, 'verify', 'verify', size, (t) => wobble(420 * MiB, t, 5), { workers: 4 });
    const bad = e.slug === 'moss-and-mortar' ? ['data/terrain.pak', 'data/audio/ambience.bank'] : [];
    if (!bad.length || !repair) {
      return { game_id: id, checked_files: 37, bad_files: bad, repaired: false, downloaded_bytes: 0 };
    }
    const damaged = Math.round(size * 0.18);
    const fetch = Math.round(damaged * 0.25);
    await this.runPhase(id, 'verify', 'harvest', damaged, (t) => wobble(360 * MiB, t, 6));
    await this.runPhase(id, 'verify', 'download', fetch, this.netRate(30 * MiB, 7), { source: 'http' });
    await this.runPhase(id, 'verify', 'assemble', damaged, (t) => wobble(200 * MiB, t, 8));
    return { game_id: id, checked_files: 37, bad_files: bad, repaired: true, downloaded_bytes: fetch };
  }

  /**
   * Two fake filesystems: everything under /mnt/usb is a small, nearly full
   * stick (to see the "will not fit" warning), the rest one large disk whose
   * free space shrinks as mock installs write files.
   */
  async diskSpace(path: string): Promise<DiskSpace> {
    await sleep(30);
    if (!path.trim()) fail('empty path');
    if (path.startsWith('/mnt/usb')) {
      return { path: '/mnt/usb', total_bytes: 16 * GiB, free_bytes: 900 * MiB, volume: 'mock-usb' };
    }
    const total = 1 * TiB - 24 * GiB;
    const free = Math.max(0, 212 * GiB - this.written);
    const existing = path.startsWith(this.settings.install_root) ? this.settings.install_root : path;
    return { path: existing, total_bytes: total, free_bytes: free, volume: 'mock-root' };
  }

  async launch(selector: string): Promise<void> {
    const e = this.resolve(selector);
    if (!e.installed) fail(`${e.slug} is not installed`);
    if (this.running.has(e.game_id)) fail(`${e.title} is already running`);
    this.running.add(e.game_id);
    setTimeout(() => {
      this.running.delete(e.game_id);
      for (const l of this.exitedListeners) l({ game_id: e.game_id, code: 0 });
    }, 5000);
  }

  async runningGames(): Promise<string[]> {
    return [...this.running];
  }

  async uninstall(selector: string, forget: boolean): Promise<void> {
    await sleep(200);
    const e = this.resolve(selector);
    this.seeds.delete(e.game_id);
    if (e.installed) this.written -= e.installed.size;
    if (forget) this.library.delete(e.game_id);
    else e.installed = null;
  }

  // ---- seeding ----

  private startSeed(gameId: string, total: number) {
    this.engineStarted = true;
    const now = Date.now();
    this.seeds.set(gameId, { since: now, at: now, uploaded: 0, total });
  }

  async startSeeding(): Promise<SeedFailure[]> {
    const failures: SeedFailure[] = [];
    if (!this.settings.seed) return failures;
    await sleep(1500); // the real engine re-verifies every install first
    for (const e of this.library.values()) {
      if (!e.installed) continue;
      const g = this.catalogFor(e);
      if (e.installed.version !== g.latest.version) {
        // Demo of a non-blocking seeding failure.
        failures.push({ slug: e.slug, reason: 'the server has no torrent for this release (yet)' });
        continue;
      }
      if (!this.seeds.has(e.game_id)) this.startSeed(e.game_id, g.latest.size);
    }
    return failures;
  }

  async seedGame(selector: string): Promise<void> {
    await sleep(600);
    const e = this.resolve(selector);
    if (!e.installed) fail(`${e.slug} is not installed`);
    const g = this.catalogFor(e);
    if (e.installed.version !== g.latest.version) fail('the server has no torrent for this release (yet)');
    this.startSeed(e.game_id, g.latest.size);
  }

  async stopSeeding(gameId: string): Promise<void> {
    this.seeds.delete(gameId);
  }

  async seedingStatus(): Promise<SeedingInfo> {
    const now = Date.now();
    const games: SeedStatus[] = [];
    let i = 0;
    for (const [game_id, s] of this.seeds) {
      // A fake upload counter: peers come and go, so the rate drifts between
      // nothing and the upload limit.
      const t = (now - s.since) / 1000;
      const interest = Math.max(0, 0.45 + 0.35 * Math.sin(t / 19 + i) + 0.25 * Math.sin(t / 5.3 + i * 2));
      const cap = this.settings.upload_limit ?? 4 * MiB;
      const rate = Math.min(cap, interest * 700 * 1024 * (0.85 + Math.random() * 0.3));
      s.uploaded = Math.min(s.total * 3, Math.round(s.uploaded + (rate * (now - s.at)) / 1000));
      s.at = now;
      i++;
      games.push({ game_id, info_hash: hex(game_id).slice(0, 40), uploaded_bytes: s.uploaded, total_bytes: s.total });
    }
    return { games, port: this.engineStarted ? this.settings.listen_port || 51413 : null };
  }

  async takeSeedFailures(): Promise<SeedFailure[]> {
    this.startup.listening = true;
    const f = this.startup.failures;
    this.startup.failures = [];
    return f;
  }

  async listCatalogs(): Promise<string[]> {
    return [...this.catalogList];
  }
  async addCatalog(server: string): Promise<string> {
    await sleep(300);
    let url: URL;
    try {
      url = new URL(server);
    } catch {
      fail(`invalid server URL: ${server}`);
    }
    const base = url.origin;
    if (!this.catalogList.includes(base)) this.catalogList.push(base);
    return url.hostname;
  }
  async removeCatalog(server: string): Promise<void> {
    this.catalogList = this.catalogList.filter((c) => c !== server);
  }

  async discover(query: string): Promise<DiscoverResult> {
    await sleep(250);
    const q = query.trim().toLowerCase();
    const games: DiscoverEntry[] = [];
    const errors: string[] = [];
    for (const server of this.catalogList) {
      const fromServer = catalogGames.filter((g) => g.server === server);
      if (!fromServer.length) {
        errors.push(`${server}: network: connection refused`);
        continue;
      }
      for (const g of fromServer) {
        if (q && !`${g.entry.title} ${g.entry.summary} ${g.entry.slug}`.toLowerCase().includes(q)) continue;
        games.push({
          server,
          server_name: g.serverName,
          in_library: this.library.has(g.entry.game_id),
          game_id: g.entry.game_id,
          slug: g.entry.slug,
          title: g.entry.title,
          summary: g.entry.summary,
          developer_pubkey: g.entry.pinned_key,
          url: `${server}/games/${g.entry.slug}`,
          icon: g.entry.icon,
          banner: g.entry.banner,
          channels: g.entry.channels,
          asset_base: ASSETS,
        });
      }
    }
    return { games, errors };
  }
}

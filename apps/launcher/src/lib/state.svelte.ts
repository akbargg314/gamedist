// Shared application state (Svelte 5 runes in a module).

import * as api from './api';
import { ApiError } from './api';
import type {
  InstallOptions,
  LibraryEntry,
  OpKind,
  Phase,
  ProgressEvent,
  SeedFailure,
  Source,
  SeedingInfo,
  SeedStatus,
  Settings,
  SettingsPatch,
  UpdateInfo,
} from './types';
import { defaultDir, formatBytes } from './format';
import { emptySnapshot, Meter, type TrafficSnapshot } from './throughput';

export type View = 'library' | 'discover' | 'downloads' | 'settings';

export interface Operation {
  game_id: string;
  title: string;
  op: OpKind;
  phase: Phase | 'starting';
  /** Download source of the current counter; null outside the download phase. */
  source: Source | null;
  done: number;
  total: number;
  started: number;
  /** Where the game is (being) installed, if known; used for disk-space checks. */
  dir: string | null;
}

export interface FinishedOp {
  game_id: string;
  title: string;
  op: OpKind;
  ok: boolean;
  message: string;
  at: number;
}

export interface Toast {
  id: number;
  kind: 'info' | 'error' | 'success';
  text: string;
}

export interface KeyChangePrompt {
  title: string;
  slug?: string;
  sourceUrl?: string;
  pinned?: string;
  presented?: string;
  message: string;
  resolve: (trust: boolean) => void;
}

class AppState {
  games = $state<LibraryEntry[]>([]);
  loaded = $state(false);
  selectedId = $state<string | null>(null);
  view = $state<View>('library');
  settings = $state<Settings | null>(null);
  ops = $state<Record<string, Operation>>({});
  history = $state<FinishedOp[]>([]);
  running = $state<string[]>([]);
  updates = $state<Record<string, UpdateInfo>>({});
  toasts = $state<Toast[]>([]);
  addDialog = $state<{ open: boolean; url: string; seq: number }>({ open: false, url: '', seq: 0 });
  keyPrompt = $state<KeyChangePrompt | null>(null);
  /** Last polled seeding status; only kept fresh while some view watches it. */
  seeding = $state<SeedingInfo>({ games: [], port: null });
  /** Transfer rates and history, republished about once a second while anything moves. */
  traffic = $state.raw<TrafficSnapshot>(emptySnapshot());

  selected = $derived(this.games.find((g) => g.game_id === this.selectedId) ?? null);
  activeOps = $derived(Object.values(this.ops));

  private toastSeq = 0;

  async init() {
    await Promise.all([
      api.onProgress((e) => this.onProgress(e)),
      api.onExited((e) => {
        this.running = this.running.filter((id) => id !== e.game_id);
        const g = this.byId(e.game_id);
        if (e.code && e.code !== 0) this.toast('error', `${g?.title ?? 'Game'} exited with code ${e.code}`);
      }),
      api.onOpenUrl((url) => this.openAddDialog(url)),
      api.onSeedFailures((f) => this.reportSeedFailures(f)),
    ]);
    try {
      [this.settings, this.running] = await Promise.all([api.getSettings(), api.runningGames()]);
      this.syncSeedPolling();
      await this.refresh();
    } catch (e) {
      this.toast('error', api.errorMessage(e));
    }
    this.loaded = true;
    for (const url of await api.takePendingLinks().catch(() => [] as string[])) this.openAddDialog(url);
    // The backend resumes seeding by itself at startup; collect what failed so far.
    void api.takeSeedFailures().then(
      (f) => this.reportSeedFailures(f),
      () => {},
    );
    // Startup update check (PLAN §5 flow 4); failures are silent per game.
    for (const g of this.games) if (g.installed) void this.checkUpdate(g.game_id, true);
  }

  byId(id: string) {
    return this.games.find((g) => g.game_id === id);
  }

  async refresh() {
    this.games = (await api.listGames()).sort((a, b) => a.title.localeCompare(b.title));
    if (this.selectedId && !this.byId(this.selectedId)) this.selectedId = null;
  }

  select(id: string | null) {
    this.selectedId = id;
    this.view = 'library';
  }

  toast(kind: Toast['kind'], text: string) {
    const id = ++this.toastSeq;
    this.toasts = [...this.toasts, { id, kind, text }];
    setTimeout(() => this.dismiss(id), kind === 'error' ? 9000 : 4500);
  }

  dismiss(id: number) {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  private lastDeepLink = { url: '', at: 0 };
  openAddDialog(url = '') {
    // The same link can arrive both via argv and the deep-link event.
    const now = Date.now();
    if (url && url === this.lastDeepLink.url && now - this.lastDeepLink.at < 2000) return;
    if (url) this.lastDeepLink = { url, at: now };
    this.addDialog = { open: true, url, seq: this.addDialog.seq + 1 };
  }

  closeAddDialog() {
    this.addDialog = { ...this.addDialog, open: false };
  }

  /** Ask the user about a key change. Resolves false on Cancel / Escape. */
  confirmKeyChange(err: ApiError, title: string): Promise<boolean> {
    return new Promise((resolve) => {
      this.keyPrompt = {
        title,
        slug: err.slug,
        sourceUrl: err.sourceUrl,
        pinned: err.pinned,
        presented: err.presented,
        message: err.message,
        resolve: (trust) => {
          this.keyPrompt = null;
          resolve(trust);
        },
      };
    });
  }

  /**
   * Add a game. On a key change, blocks on the warning dialog and only
   * re-calls with accept_key_change=true after explicit confirmation.
   * Returns null if the user declined.
   */
  async addGame(url: string): Promise<LibraryEntry | null> {
    try {
      return await this.afterAdd(await api.addGame(url, false));
    } catch (e) {
      if (!(e instanceof ApiError) || !e.isKeyChanged) throw e;
      const trust = await this.confirmKeyChange(e, e.slug ?? url);
      if (!trust) return null;
      return await this.afterAdd(await api.addGame(url, true));
    }
  }

  private async afterAdd(entry: LibraryEntry) {
    await this.refresh();
    return entry;
  }

  /**
   * Run an operation that may hit a key change (update checks, installs).
   * The only API that can accept a new key is add_game, so on confirmation we
   * re-add from the source that presented the key, then retry once.
   */
  private async keyGuarded<T>(entry: LibraryEntry, run: () => Promise<T>): Promise<T | null> {
    try {
      return await run();
    } catch (e) {
      if (!(e instanceof ApiError) || !e.isKeyChanged) throw e;
      const trust = await this.confirmKeyChange(e, entry.title);
      if (!trust) return null;
      const src = e.sourceUrl ?? `${entry.sources[0]}/games/${entry.slug}`;
      await api.addGame(src, true);
      await this.refresh();
      return await run();
    }
  }

  async checkUpdate(id: string, quiet = false): Promise<UpdateInfo | null> {
    const entry = this.byId(id);
    if (!entry) return null;
    try {
      const info = quiet ? await api.checkUpdate(id) : await this.keyGuarded(entry, () => api.checkUpdate(id));
      if (info) this.updates = { ...this.updates, [id]: info };
      return info;
    } catch (e) {
      if (!quiet) this.toast('error', `Update check failed: ${api.errorMessage(e)}`);
      return null;
    }
  }

  private begin(entry: LibraryEntry, op: OpKind, dir: string | null) {
    const now = Date.now();
    this.ops = {
      ...this.ops,
      [entry.game_id]: { game_id: entry.game_id, title: entry.title, op, phase: 'starting', source: null, done: 0, total: 0, started: now, dir },
    };
    this.meter.start(entry.game_id, now);
    this.startSampling();
  }

  private finish(entry: LibraryEntry, op: OpKind, ok: boolean, message: string) {
    const { [entry.game_id]: _, ...rest } = this.ops;
    this.ops = rest;
    this.meter.end(entry.game_id, Date.now());
    this.publishTraffic();
    this.history = [{ game_id: entry.game_id, title: entry.title, op, ok, message, at: Date.now() }, ...this.history].slice(0, 30);
    this.toast(ok ? 'success' : 'error', message);
  }

  private onProgress(e: ProgressEvent) {
    const cur = this.ops[e.game_id];
    if (!cur) return;
    const source = e.phase === 'download' ? (e.source ?? null) : null;
    // The meter ignores out-of-order disk reports from worker threads; show its value.
    const done = this.meter.progress(e.game_id, e.phase, e.done, e.total, Date.now(), source);
    this.ops = { ...this.ops, [e.game_id]: { ...cur, op: e.op, phase: e.phase, source, done, total: e.total } };
  }

  async install(id: string, opts: InstallOptions = {}) {
    const entry = this.byId(id);
    if (!entry || this.ops[id]) return;
    this.begin(entry, 'install', opts.dir || entry.installed?.dir || defaultDir(this.settings?.install_root, entry.slug) || null);
    try {
      const r = await this.keyGuarded(entry, () => api.install(id, opts));
      if (!r) return this.finish(entry, 'install', false, `${entry.title}: install cancelled`);
      const fetched =
        `${formatBytes(r.downloaded_bytes)} downloaded` +
        (r.torrent_bytes > 0 ? `, ${formatBytes(r.torrent_bytes)} from peers` : '');
      const msg = r.up_to_date
        ? `${entry.title} ${r.version} is already up to date`
        : r.previous_version
          ? `${entry.title} updated ${r.previous_version} → ${r.version} (${fetched})`
          : `${entry.title} ${r.version} installed (${fetched})`;
      this.finish(entry, 'install', true, msg);
    } catch (e) {
      this.finish(entry, 'install', false, `${entry.title}: ${api.errorMessage(e)}`);
    }
    await this.refresh().catch(() => {});
    if (this.seedWatchers) void this.refreshSeeding();
    await this.checkUpdate(id, true);
  }

  async verify(id: string) {
    const entry = this.byId(id);
    if (!entry || this.ops[id]) return;
    this.begin(entry, 'verify', entry.installed?.dir ?? null);
    try {
      const r = await api.verify(id, true);
      const msg = r.bad_files.length
        ? `${entry.title}: ${r.bad_files.length} damaged file(s) ${r.repaired ? `repaired (${formatBytes(r.downloaded_bytes)} downloaded)` : 'found'}`
        : `${entry.title}: all ${r.checked_files} files OK`;
      this.finish(entry, 'verify', true, msg);
    } catch (e) {
      this.finish(entry, 'verify', false, `${entry.title}: verify failed: ${api.errorMessage(e)}`);
    }
  }

  async launch(id: string) {
    const entry = this.byId(id);
    if (!entry) return;
    try {
      await api.launch(id);
      this.running = [...this.running, id];
    } catch (e) {
      this.toast('error', `Could not start ${entry.title}: ${api.errorMessage(e)}`);
    }
  }

  async uninstall(id: string, forget: boolean) {
    const entry = this.byId(id);
    if (!entry) return;
    try {
      await api.uninstall(id, forget);
      const { [id]: _, ...rest } = this.updates;
      this.updates = rest;
      if (forget && this.selectedId === id) this.selectedId = null;
      await this.refresh();
      if (this.seedWatchers) void this.refreshSeeding();
      this.toast('info', forget ? `${entry.title} removed from library` : `${entry.title} uninstalled`);
    } catch (e) {
      this.toast('error', `Uninstall failed: ${api.errorMessage(e)}`);
    }
  }

  /** Save only the edited fields. Turning seeding on starts it in the background. */
  async saveSettings(patch: SettingsPatch) {
    const wasSeeding = this.settings?.seed ?? false;
    this.settings = await api.updateSettings(patch);
    this.syncSeedPolling();
    if (!wasSeeding && this.settings.seed) void this.startSeeding();
    else if (wasSeeding && !this.settings.seed && this.seedWatchers) void this.refreshSeeding();
  }

  // ---- seeding ----

  private reportSeedFailures(failures: SeedFailure[]) {
    for (const f of failures) {
      const title = this.games.find((g) => g.slug === f.slug)?.title ?? f.slug;
      this.toast('error', `Not seeding ${title}: ${f.reason}`);
    }
  }

  /** Seed all installed games; failures become toasts, nothing blocks. */
  async startSeeding() {
    try {
      this.reportSeedFailures(await api.startSeeding());
    } catch (e) {
      this.toast('error', `Seeding failed: ${api.errorMessage(e)}`);
    }
    if (this.seedWatchers) await this.refreshSeeding();
  }

  async seedGame(id: string) {
    const entry = this.byId(id);
    if (!entry) return;
    try {
      await api.seedGame(id);
    } catch (e) {
      this.toast('error', `Not seeding ${entry.title}: ${api.errorMessage(e)}`);
    }
    await this.refreshSeeding();
  }

  async stopSeeding(id: string) {
    try {
      await api.stopSeeding(id);
    } catch (e) {
      this.toast('error', api.errorMessage(e));
    }
    await this.refreshSeeding();
  }

  seedStatus(id: string): SeedStatus | undefined {
    return this.seeding.games.find((s) => s.game_id === id);
  }

  async refreshSeeding() {
    try {
      this.seeding = await api.seedingStatus();
    } catch {
      // Status is cosmetic; keep the last value.
      return;
    }
    const now = Date.now();
    for (const g of this.seeding.games) this.meter.counter(g.game_id, g.uploaded_bytes, now);
    this.meter.retainCounters(
      this.seeding.games.map((g) => g.game_id),
      now,
    );
    if (this.seeding.games.length) this.startSampling();
  }

  /**
   * While seeding is on, keep polling its status even when no view shows it,
   * so the upload graph has history. Off = no polling at all.
   */
  private bgSeedStop: (() => void) | null = null;
  private syncSeedPolling() {
    const want = this.settings?.seed ?? false;
    if (want && !this.bgSeedStop) this.bgSeedStop = this.watchSeeding();
    else if (!want && this.bgSeedStop) {
      this.bgSeedStop();
      this.bgSeedStop = null;
      // One last look, so stopped seeds (and the sampler) do not linger.
      void this.refreshSeeding();
    }
  }

  // ---- transfer sampling ----

  private meter = new Meter();
  private sampleTimer: ReturnType<typeof setInterval> | null = null;
  private trafficWatchers = 0;

  /** Tick the meter once a second while anything moves (or a view shows it). */
  private startSampling() {
    if (this.sampleTimer) return;
    this.sampleTimer = setInterval(() => this.sample(), 1000);
    this.publishTraffic();
  }

  private sample() {
    const now = Date.now();
    this.meter.tick(now);
    this.publishTraffic(now);
    const busy = Object.keys(this.ops).length > 0 || this.seeding.games.length > 0 || this.trafficWatchers > 0;
    if (!busy && this.meter.idle(now) && this.sampleTimer) {
      clearInterval(this.sampleTimer);
      this.sampleTimer = null;
    }
  }

  private publishTraffic(now = Date.now()) {
    // Nobody can see it while the window is hidden; the meter keeps counting.
    if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
    this.traffic = this.meter.snapshot(now);
  }

  /** Keep the traffic snapshot fresh while a view shows it. Use as `$effect(() => app.watchTraffic())`. */
  watchTraffic(): () => void {
    this.trafficWatchers++;
    this.startSampling();
    this.publishTraffic();
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      this.trafficWatchers--;
    };
  }

  private seedWatchers = 0;
  private seedTimer: ReturnType<typeof setInterval> | null = null;

  /**
   * Poll seeding status while at least one caller watches it (and the window
   * is visible). Returns the stop function; use as `$effect(() => app.watchSeeding())`.
   */
  watchSeeding(intervalMs = 3000): () => void {
    if (this.seedWatchers++ === 0) {
      void this.refreshSeeding();
      this.seedTimer = setInterval(() => {
        if (typeof document === 'undefined' || document.visibilityState === 'visible') void this.refreshSeeding();
      }, intervalMs);
    }
    let stopped = false;
    return () => {
      if (stopped) return;
      stopped = true;
      if (--this.seedWatchers === 0 && this.seedTimer) {
        clearInterval(this.seedTimer);
        this.seedTimer = null;
      }
    };
  }
}

export const app = new AppState();

export type GameStatus = 'not-installed' | 'busy' | 'running' | 'update' | 'installed';

export function statusOf(g: LibraryEntry): GameStatus {
  if (app.ops[g.game_id]) return 'busy';
  if (app.running.includes(g.game_id)) return 'running';
  if (!g.installed) return 'not-installed';
  if (app.updates[g.game_id]?.update_available) return 'update';
  return 'installed';
}

// The one typed gateway to the backend. Inside Tauri every call is an IPC
// `invoke` into src-tauri/src/commands.rs; in a plain browser (vite dev) the
// calls go to an in-memory mock so the UI can be developed and screenshotted.

import type {
  BackendError,
  DiscoverResult,
  DiskSpace,
  ExitedEvent,
  InstallOptions,
  InstallReport,
  LibraryEntry,
  ProgressEvent,
  SeedFailure,
  SeedingInfo,
  Settings,
  SettingsPatch,
  UpdateInfo,
  VerifyReport,
} from './types';
import { MockBackend } from './mock';

export const isTauri: boolean = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const EVENT_PROGRESS = 'gamedist://progress';
export const EVENT_EXITED = 'gamedist://exited';
export const EVENT_OPEN_URL = 'gamedist://open-url';
export const EVENT_SEED_FAILURES = 'gamedist://seed-failures';

/** Error thrown by every API function. `kind === 'key_changed'` needs user confirmation. */
export class ApiError extends Error {
  readonly kind: BackendError['kind'];
  readonly pinned?: string;
  readonly presented?: string;
  readonly slug?: string;
  readonly sourceUrl?: string;

  constructor(e: BackendError) {
    super(e.message);
    this.name = 'ApiError';
    this.kind = e.kind;
    this.pinned = e.pinned;
    this.presented = e.presented;
    this.slug = e.slug;
    this.sourceUrl = e.source_url;
  }

  get isKeyChanged(): boolean {
    return this.kind === 'key_changed';
  }
}

function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  if (e && typeof e === 'object' && 'kind' in e && 'message' in e) return new ApiError(e as BackendError);
  if (typeof e === 'string') return new ApiError({ kind: 'error', message: e });
  if (e instanceof Error) return new ApiError({ kind: 'error', message: e.message });
  return new ApiError({ kind: 'error', message: String(e) });
}

export function errorMessage(e: unknown): string {
  return toApiError(e).message;
}

let mock: MockBackend | null = null;
function m(): MockBackend {
  return (mock ??= new MockBackend());
}

async function call<T>(cmd: string, args: Record<string, unknown>, fallback: () => Promise<T>): Promise<T> {
  try {
    if (isTauri) {
      const { invoke } = await import('@tauri-apps/api/core');
      return await invoke<T>(cmd, args);
    }
    return await fallback();
  } catch (e) {
    throw toApiError(e);
  }
}

// ---- library & settings -------------------------------------------------

export const getSettings = () => call<Settings>('get_settings', {}, () => m().getSettings());

/** Apply only the given fields; everything else keeps its current value. Returns the saved settings. */
export const updateSettings = (patch: SettingsPatch) =>
  call<Settings>('update_settings', { patch }, () => m().updateSettings(patch));

export const listGames = () => call<LibraryEntry[]>('list_games', {}, () => m().listGames());

export const findGame = (selector: string) =>
  call<LibraryEntry>('find_game', { selector }, () => m().findGame(selector));

/** Fetch and pin a game from its URL. Rejects with `kind: 'key_changed'` if the key differs from the pin. */
export const addGame = (url: string, acceptKeyChange = false) =>
  call<LibraryEntry>('add_game', { url, acceptKeyChange }, () => m().addGame(url, acceptKeyChange));

export const checkUpdate = (selector: string) =>
  call<UpdateInfo>('check_update', { selector }, () => m().checkUpdate(selector));

/** Installs or updates; progress arrives through `onProgress` with op "install". */
export const install = (selector: string, opts: InstallOptions = {}) =>
  call<InstallReport>(
    'install_game',
    {
      selector,
      opts: {
        dir: opts.dir ?? null,
        platform: opts.platform ?? null,
        channel: opts.channel ?? null,
        version: opts.version ?? null,
        allow_downgrade: opts.allow_downgrade ?? false,
      },
    },
    () => m().install(selector, opts),
  );

export const verify = (selector: string, repair = true) =>
  call<VerifyReport>('verify_game', { selector, repair }, () => m().verify(selector, repair));

export const launch = (selector: string, extraArgs: string[] = []) =>
  call<void>('launch_game', { selector, extraArgs }, () => m().launch(selector));

export const runningGames = () => call<string[]>('running_games', {}, () => m().runningGames());

export const uninstall = (selector: string, forget = false) =>
  call<void>('uninstall_game', { selector, forget }, () => m().uninstall(selector, forget));

// ---- seeding -------------------------------------------------------------

/** Seed every installed game (no-op when seeding is off). Slow; resolves with the games that failed. */
export const startSeeding = () => call<SeedFailure[]>('start_seeding', {}, () => m().startSeeding());

export const seedGame = (selector: string) => call<void>('seed_game', { selector }, () => m().seedGame(selector));

export const stopSeeding = (gameId: string) =>
  call<void>('stop_seeding', { gameId }, () => m().stopSeeding(gameId));

export const seedingStatus = () => call<SeedingInfo>('seeding_status', {}, () => m().seedingStatus());

/** Startup seeding failures that happened before the UI subscribed to `onSeedFailures`. */
export const takeSeedFailures = () => call<SeedFailure[]>('take_seed_failures', {}, () => m().takeSeedFailures());

// ---- disk ----------------------------------------------------------------

/** Free/total space of the filesystem holding `path`; the path need not exist yet. */
export const diskSpace = (path: string) => call<DiskSpace>('disk_space', { path }, () => m().diskSpace(path));

// ---- catalogs ------------------------------------------------------------

export const listCatalogs = () => call<string[]>('list_catalogs', {}, () => m().listCatalogs());

export const addCatalog = (server: string) =>
  call<string>('add_catalog', { server }, () => m().addCatalog(server));

export const removeCatalog = (server: string) =>
  call<void>('remove_catalog', { server }, () => m().removeCatalog(server));

export const discover = (query: string) =>
  call<DiscoverResult>('discover', { query }, () => m().discover(query));

// ---- deep links ----------------------------------------------------------

/** URLs the app was started with (or received before the UI was listening). */
export const takePendingLinks = () => call<string[]>('take_pending_links', {}, async () => {
  const u = new URL(window.location.href).searchParams.get('open');
  return u ? [u] : [];
});

// ---- events --------------------------------------------------------------

type Unlisten = () => void;

async function listenTauri<T>(event: string, cb: (payload: T) => void): Promise<Unlisten> {
  const { listen } = await import('@tauri-apps/api/event');
  return listen<T>(event, (e) => cb(e.payload));
}

export async function onProgress(cb: (e: ProgressEvent) => void): Promise<Unlisten> {
  if (isTauri) return listenTauri(EVENT_PROGRESS, cb);
  const off = m().onProgress(cb);
  return () => void off();
}

export async function onExited(cb: (e: ExitedEvent) => void): Promise<Unlisten> {
  if (isTauri) return listenTauri(EVENT_EXITED, cb);
  const off = m().onExited(cb);
  return () => void off();
}

/** Fires when a `gamedist://` link is opened while the app is running. */
export async function onOpenUrl(cb: (url: string) => void): Promise<Unlisten> {
  if (isTauri) return listenTauri<string[]>(EVENT_OPEN_URL, (urls) => urls.forEach(cb));
  return () => {};
}

/** Startup seeding failures that finish after `takeSeedFailures` was called. */
export async function onSeedFailures(cb: (failures: SeedFailure[]) => void): Promise<Unlisten> {
  if (isTauri) return listenTauri(EVENT_SEED_FAILURES, cb);
  const off = m().onSeedFailures(cb);
  return () => void off();
}

// ---- OS integration ------------------------------------------------------

/** Folder picker; returns null when cancelled. */
export async function pickDirectory(defaultPath?: string, title = 'Choose a folder'): Promise<string | null> {
  if (isTauri) {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ directory: true, multiple: false, defaultPath, title });
    return typeof r === 'string' ? r : null;
  }
  const r = window.prompt(title, defaultPath ?? '');
  return r && r.trim() ? r.trim() : null;
}

/** File picker (e.g. Wine runner binary); returns null when cancelled. */
export async function pickFile(defaultPath?: string, title = 'Choose a file'): Promise<string | null> {
  if (isTauri) {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ directory: false, multiple: false, defaultPath, title });
    return typeof r === 'string' ? r : null;
  }
  const r = window.prompt(title, defaultPath ?? '');
  return r && r.trim() ? r.trim() : null;
}

/** Open an http(s)/mailto link in the system browser. Other schemes are refused. */
export async function openExternal(url: string): Promise<void> {
  if (!/^(https?:|mailto:)/i.test(url)) return;
  if (isTauri) {
    const { openUrl } = await import('@tauri-apps/plugin-opener');
    await openUrl(url);
  } else {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}

// ---- helpers -------------------------------------------------------------

export function assetUrl(base: string | undefined, id: string | null | undefined): string | null {
  if (!id || !base) return null;
  return base + id;
}

/** Image base for a catalog entry: explicit `asset_base`, else `<origin>/chunks/`. */
export function discoverAssetBase(entry: { asset_base: string; url: string }): string | undefined {
  if (entry.asset_base) return entry.asset_base;
  try {
    return new URL(entry.url).origin + '/chunks/';
  } catch {
    return undefined;
  }
}

/** True for URLs the Add-game flow accepts. */
export function looksLikeGameUrl(s: string): boolean {
  return /^(https?|gamedist|gamedist\+http):\/\/[^/\s]+\/games\/[a-z0-9][a-z0-9-]*\/?$/.test(s.trim());
}

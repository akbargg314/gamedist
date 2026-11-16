// Wire types mirrored from the `gamedist-launcher` crate (serde JSON shapes).

export type Transport = 'direct' | 'torrent-direct' | 'torrent';
export type Phase = 'harvest' | 'download' | 'assemble' | 'verify' | 'done';
/** Where the bytes of a `download` progress event come from. */
export type Source = 'http' | 'peers';
export type OpKind = 'install' | 'verify';

export interface Settings {
  install_root: string;
  /** bytes per second; null = unlimited */
  bandwidth_limit: number | null;
  concurrency: number;
  keep_chunks: boolean;
  transport: Transport;
  seed: boolean;
  /** bytes per second; null = unlimited */
  upload_limit: number | null;
  wine: string | null;
  /** TCP port for incoming BitTorrent connections; 0 = any free port. */
  listen_port: number;
  /** Not edited by the UI; untouched by saves because the UI sends a SettingsPatch. */
  torrent_stall_secs: number;
}

/** The subset of settings the UI edits; sent as a patch so other fields survive. */
export type SettingsPatch = Partial<
  Pick<
    Settings,
    | 'install_root'
    | 'bandwidth_limit'
    | 'concurrency'
    | 'keep_chunks'
    | 'transport'
    | 'seed'
    | 'upload_limit'
    | 'wine'
    | 'listen_port'
  >
>;

export interface Installed {
  version: string;
  platform: string;
  dir: string;
  installed_at: string;
  /** Size of the installed files in bytes, from the signed manifest; 0 = unknown. */
  size: number;
}

export interface LibraryEntry {
  game_id: string;
  slug: string;
  title: string;
  summary: string;
  description_md: string;
  sources: string[];
  /** "ed25519:…" */
  pinned_key: string;
  channel: string;
  channels: Record<string, string>;
  icon: string | null;
  banner: string | null;
  screenshots: string[];
  asset_base: string;
  installed: Installed | null;
}

export interface InstallOptions {
  dir?: string | null;
  platform?: string | null;
  channel?: string | null;
  version?: string | null;
  allow_downgrade?: boolean;
}

export interface InstallReport {
  game_id: string;
  version: string;
  previous_version: string | null;
  platform: string;
  dir: string;
  downloaded_chunks: number;
  downloaded_bytes: number;
  harvested_bytes: number;
  /** Bytes obtained (and verified) from BitTorrent peers. */
  torrent_bytes: number;
  files_written: number;
  files_deleted: number;
  up_to_date: boolean;
  /** The install is now being seeded. */
  seeding: boolean;
}

export interface SeedStatus {
  game_id: string;
  info_hash: string;
  uploaded_bytes: number;
  total_bytes: number;
}

export interface SeedingInfo {
  games: SeedStatus[];
  /** BitTorrent listen port once the engine runs, else null. */
  port: number | null;
}

export interface SeedFailure {
  slug: string;
  reason: string;
}

export interface UpdateInfo {
  game_id: string;
  installed: string | null;
  latest: string;
  update_available: boolean;
  download_bytes: number;
  total_bytes: number;
  changelog_md: string;
}

export interface VerifyReport {
  game_id: string;
  checked_files: number;
  bad_files: string[];
  repaired: boolean;
  downloaded_bytes: number;
}

export interface DiscoverEntry {
  server: string;
  server_name: string;
  in_library: boolean;
  game_id: string;
  slug: string;
  title: string;
  summary: string;
  developer_pubkey: string;
  url: string;
  /** Omitted by the server when unset. */
  icon?: string | null;
  banner?: string | null;
  channels: Record<string, string>;
  /** Image URL prefix (`asset_base + chunk id`); when empty, `<origin of url>/chunks/` is used. */
  asset_base: string;
}

export interface DiscoverResult {
  games: DiscoverEntry[];
  errors: string[];
}

/** `disk_space`: the filesystem holding a path (or its nearest existing ancestor). */
export interface DiskSpace {
  /** The existing directory that was measured. */
  path: string;
  total_bytes: number;
  /** Available to this user. */
  free_bytes: number;
  /** Same value = same filesystem. */
  volume: string;
}

/**
 * `done` / `total` are bytes of the current phase. In the `download` phase
 * they count one `source` only: a torrent install reports "peers" with the
 * torrent's totals, then "http" with its own (smaller) total for the rest.
 * `verify` counts bytes checked (total = installed size), `harvest` bytes
 * read from the installed files, `assemble` bytes written.
 */
export interface ProgressEvent {
  game_id: string;
  op: OpKind;
  phase: Phase;
  /** "http" | "peers" in the download phase, null in every other phase. */
  source: Source | null;
  done: number;
  total: number;
}

export interface ExitedEvent {
  game_id: string;
  code: number | null;
}

/** Error shape returned by every Tauri command. */
export interface BackendError {
  kind: 'key_changed' | 'error';
  message: string;
  pinned?: string;
  presented?: string;
  slug?: string;
  source_url?: string;
}

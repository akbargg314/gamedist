const UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];

export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  if (n < 1024) return `${n} B`;
  let i = 0;
  let v = n;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${UNITS[i]}`;
}

export function formatRate(n: number | null | undefined): string {
  return n ? `${formatBytes(n)}/s` : 'Unlimited';
}

/** A measured transfer rate, e.g. "12.4 MiB/s"; "0 B/s" when idle. */
export function formatSpeed(bytesPerSec: number | null | undefined): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec)) return '—';
  return `${formatBytes(bytesPerSec < 1024 ? Math.round(bytesPerSec) : bytesPerSec)}/s`;
}

/** Rough remaining time: "45 s", "3 min 20 s", "1 h 5 min", "2 d 3 h". */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return '—';
  const s = Math.max(1, Math.round(seconds));
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 10) return `${m} min ${s % 60} s`;
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h} h ${m % 60} min`;
  return `${Math.floor(h / 24)} d ${h % 24} h`;
}

export function percent(done: number, total: number): number {
  if (!total) return 0;
  return Math.max(0, Math.min(100, (done / total) * 100));
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return '—';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleDateString(undefined, { dateStyle: 'medium' });
}

export const PHASE_LABEL: Record<string, string> = {
  harvest: 'Reusing installed files',
  download: 'Downloading',
  assemble: 'Writing files',
  verify: 'Checking files',
  done: 'Finishing',
};

/** Short label of what an operation's progress counter measures right now. */
export function stepLabel(op: { op: string; phase: string; source?: string | null }): string {
  if (op.phase === 'download') {
    if (op.source === 'peers') return 'Downloading from other players';
    if (op.source === 'http') return op.op === 'verify' ? 'Downloading repairs from the server' : 'Downloading from the server';
    return 'Downloading';
  }
  if (op.phase === 'harvest') return op.op === 'verify' ? 'Reading intact parts of damaged files' : 'Reusing installed files';
  if (op.phase === 'assemble') return op.op === 'verify' ? 'Writing repaired files' : 'Writing files';
  if (op.phase === 'verify') return 'Checking installed files';
  if (op.phase === 'starting') return 'Preparing…';
  return PHASE_LABEL[op.phase] ?? op.phase;
}

/** Share of `part` in `whole` as "68%" ("<1%", ">99%" near the ends); null when `part` is 0. */
export function share(part: number, whole: number): string | null {
  if (!(whole > 0) || !(part > 0)) return null;
  const p = Math.round((Math.min(part, whole) / whole) * 100);
  if (p === 0) return '<1%';
  if (p === 100 && part < whole) return '>99%';
  return `${p}%`;
}

/** Stable hue (0–359) from a string, used for placeholder art. */
export function hueOf(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0;
  return h % 360;
}

export function hostOf(url: string): string {
  try {
    return new URL(url.replace(/^gamedist(\+http)?:/, 'https:')).host;
  } catch {
    return url;
  }
}

/** `<root>/<slug>`, using the separator style of `root`. */
export function defaultDir(root: string | undefined, slug: string): string {
  if (!root) return '';
  const sep = root.includes('\\') && !root.includes('/') ? '\\' : '/';
  return root.replace(/[\\/]+$/, '') + sep + slug;
}

/** Upload ratio (uploaded / size) for display, e.g. "0.42". */
export function seedRatio(s: { uploaded_bytes: number; total_bytes: number }): string {
  if (!s.total_bytes) return '0.00';
  return (s.uploaded_bytes / s.total_bytes).toFixed(2);
}

/** Size of an installed game in bytes (from its signed manifest); null when unknown (reported as 0). */
export function installedSize(g: { installed: { size?: number } | null }): number | null {
  const n = g.installed?.size;
  return n && n > 0 ? n : null;
}

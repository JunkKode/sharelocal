/** Presentation helpers. Pure functions so they can be unit tested. */

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

/**
 * Byte sizes at three significant figures, matching `format_bytes` in the Rust
 * core so the two never disagree on screen.
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes < 1024) return `${Math.floor(bytes)} B`;

  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const suffix = UNITS[unit];
  if (value >= 100) return `${value.toFixed(0)} ${suffix}`;
  if (value >= 10) return `${value.toFixed(1)} ${suffix}`;
  return `${value.toFixed(2)} ${suffix}`;
}

/** Transfer rate. Shown in MB/s because that is what people compare. */
export function formatSpeed(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return "—";
  const mb = bytesPerSecond / (1024 * 1024);
  if (mb >= 100) return `${mb.toFixed(0)} MB/s`;
  if (mb >= 10) return `${mb.toFixed(1)} MB/s`;
  if (mb >= 1) return `${mb.toFixed(1)} MB/s`;
  const kb = bytesPerSecond / 1024;
  if (kb >= 1) return `${kb.toFixed(0)} KB/s`;
  return `${Math.round(bytesPerSecond)} B/s`;
}

/** `2m 10s`, `1h 04m`, `12s`. */
export function formatDuration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "—";
  const total = Math.round(seconds);
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  const secs = total % 60;
  if (minutes < 60) return `${minutes}m ${String(secs).padStart(2, "0")}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

export function formatEta(seconds: number | null | undefined): string {
  const text = formatDuration(seconds);
  return text === "—" ? "—" : `ETA ${text}`;
}

export function formatPercent(done: number, total: number): string {
  if (total <= 0) return "0%";
  return `${Math.min(100, Math.floor((done / total) * 100))}%`;
}

export function percent(done: number, total: number): number {
  if (total <= 0) return 0;
  return Math.max(0, Math.min(100, (done / total) * 100));
}

/** `Today`, `Yesterday`, or a date — the grouping used by the history list. */
export function dayLabel(ms: number, now = Date.now()): string {
  const date = new Date(ms);
  const today = new Date(now);
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  const startOfDate = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const dayMs = 24 * 60 * 60 * 1000;

  if (startOfDate === startOfToday) return "Today";
  if (startOfDate === startOfToday - dayMs) return "Yesterday";
  if (startOfToday - startOfDate < 7 * dayMs) {
    return date.toLocaleDateString(undefined, { weekday: "long" });
  }
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
}

export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** The last path segment, for showing a file name from a relative path. */
export function baseName(relPath: string): string {
  const parts = relPath.split("/").filter(Boolean);
  return parts.length ? parts[parts.length - 1]! : relPath;
}

/** A short, human label for a set of files. */
export function describeFiles(files: { rel_path: string }[]): string {
  if (files.length === 0) return "No files";
  if (files.length === 1) return baseName(files[0]!.rel_path);
  const first = files[0]!.rel_path.split("/")[0] || baseName(files[0]!.rel_path);
  return `${first} + ${files.length - 1} more`;
}

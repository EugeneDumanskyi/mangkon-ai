import type { CatalogModel } from "../ipc/bindings/CatalogModel";

/** Bytes in GB (10^9, as the hardware check counts them): one decimal below 10 GB. */
export function gb(bytes: number): string {
  const v = bytes / 1e9;
  if (v < 10) {
    const s = v.toFixed(1);
    return s.endsWith(".0") ? s.slice(0, -2) : s;
  }
  return String(Math.round(v));
}

/** A size for the catalog: MB below 1 GB, else GB. */
export function size(bytes: number): string {
  return bytes < 1e9 ? `${String(Math.round(bytes / 1e6))} MB` : `${gb(bytes)} GB`;
}

/** Time left, as "3 min 20 s", "45 s" or "1 h 5 min". */
export function timeLeft(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${String(s)} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${String(m)} min ${String(s % 60)} s`;
  return `${String(Math.floor(m / 60))} h ${String(m % 60)} min`;
}

/** Download speed in MB/s. */
export function speed(bps: number): string {
  return `${(bps / 1e6).toFixed(1)} MB/s`;
}

/** Percent done, 0–100. */
export function percent(m: CatalogModel): number {
  return m.bytes > 0 ? Math.min(100, Math.floor((m.bytesDone / m.bytes) * 100)) : 0;
}

export function roleLabel(m: CatalogModel): string {
  return m.role === "extraction" ? "People and organizations" : "Meaning search";
}

/** The state as the row names it. */
export function stateLabel(m: CatalogModel): string {
  switch (m.state) {
    case "available":
      return "Not downloaded";
    case "downloading":
      return "Downloading";
    case "paused":
      return "Paused";
    case "interrupted":
      return "Interrupted";
    case "verifying":
      return "Checking the download…";
    case "installed":
      return "Installed";
  }
}

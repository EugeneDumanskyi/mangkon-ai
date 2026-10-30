// Pure labels of the progress panel, the documents list and the add summary (US-03.1, US-02.8,
// US-01.3 AC5).
import type { Eta } from "../ipc/bindings/Eta";
import type { UiEvent } from "../ipc/bindings/UiEvent";
import { plural } from "./labels";

/** "Estimating…" before 5 files are done (US-03.1 AC2), then "about 3 min left". */
export function etaLabel(eta: Eta): string | null {
  switch (eta.kind) {
    case "estimating":
      return "Estimating…";
    case "none":
      return null;
    case "left": {
      const s = eta.seconds;
      if (s < 60) return "less than a minute left";
      if (s < 3600) return `about ${String(Math.round(s / 60))} min left`;
      const h = Math.floor(s / 3600);
      const m = Math.round((s % 3600) / 60);
      return m === 0 ? `about ${String(h)} h left` : `about ${String(h)} h ${String(m)} min left`;
    }
  }
}

/** What the documents list calls each status. */
export const STATUS_LABELS: Record<string, string> = {
  queued: "Queued",
  processing: "Processing",
  done: "Done",
  failed: "Failed",
  not_processed: "Not processed",
  missing: "Missing",
};

export function statusLabel(status: string): string {
  return STATUS_LABELS[status] ?? status;
}

/** Whether a row offers Retry (US-02.7 AC3). */
export function canRetry(status: string): boolean {
  return status === "failed" || status === "not_processed";
}

/** "812 B", "4.2 KB", "1.3 MB", "2.0 GB" (powers of 1024). */
export function sizeLabel(bytes: number): string {
  if (bytes < 1024) return `${String(bytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i] ?? "TB"}`;
}

/** "12 pages", "1 row", "3 messages"; empty when not known. */
export function unitsLabel(units: number | null, unit: string | null): string {
  return units === null || unit === null ? "" : plural(units, unit.replace(/s$/, ""), unit);
}

type Finished = Extract<UiEvent, { kind: "ingestFinished" }>;

/**
 * The add summary (US-01.3 AC5): "3 added, 1 skipped, 2 duplicates". `null` when the run added
 * and skipped nothing (a re-scan on open).
 */
export function addSummary(e: Finished): string | null {
  if (e.added === 0 && e.skipped === 0) return null;
  return `${e.added.toLocaleString()} added, ${e.skipped.toLocaleString()} skipped, ${plural(
    e.duplicates,
    "duplicate",
  )}`;
}

/** The skipped files as listed under the summary: "notes.xyz — unsupported file type". */
export function skippedLines(e: Finished): string[] {
  const lines = e.skippedFiles.map((f) => `${f.path || "(file)"} — ${f.reason}`);
  const more = e.skipped - e.skippedFiles.length;
  if (more > 0) lines.push(`and ${more.toLocaleString()} more`);
  return lines;
}

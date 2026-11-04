import { describe, expect, it } from "vitest";
import type { UiEvent } from "../ipc/bindings/UiEvent";
import { staleKeys } from "./events";
import {
  addSummary,
  canRetry,
  etaLabel,
  sizeLabel,
  skippedLines,
  statusLabel,
  unitsLabel,
} from "./progress";

type Finished = Extract<UiEvent, { kind: "ingestFinished" }>;
const finished = (over: Partial<Finished> = {}): Finished => ({
  kind: "ingestFinished",
  done: 0,
  failed: 0,
  error: null,
  interrupted: null,
  added: 0,
  duplicates: 0,
  skipped: 0,
  skippedFiles: [],
  ...over,
});

describe("ETA (US-03.1 AC2)", () => {
  it("says Estimating… until 5 files are done, nothing when idle", () => {
    expect(etaLabel({ kind: "estimating" })).toBe("Estimating…");
    expect(etaLabel({ kind: "none" })).toBeNull();
  });

  it("rounds the time left for people", () => {
    expect(etaLabel({ kind: "left", seconds: 20 })).toBe("less than a minute left");
    expect(etaLabel({ kind: "left", seconds: 190 })).toBe("about 3 min left");
    expect(etaLabel({ kind: "left", seconds: 7200 })).toBe("about 2 h left");
    expect(etaLabel({ kind: "left", seconds: 5400 })).toBe("about 1 h 30 min left");
  });
});

describe("documents list labels (US-02.8, US-02.7)", () => {
  it("names every status and offers retry only for failed and not processed", () => {
    expect(statusLabel("not_processed")).toBe("Not processed");
    expect(statusLabel("missing")).toBe("Missing");
    expect(canRetry("failed")).toBe(true);
    expect(canRetry("not_processed")).toBe(true);
    expect(canRetry("done")).toBe(false);
    expect(canRetry("missing")).toBe(false);
  });

  it("shows sizes and units", () => {
    expect(sizeLabel(812)).toBe("812 B");
    expect(sizeLabel(4300)).toBe("4.2 KB");
    expect(sizeLabel(1_363_149)).toBe("1.3 MB");
    expect(unitsLabel(1, "pages")).toBe("1 page");
    expect(unitsLabel(12, "rows")).toBe("12 rows");
    expect(unitsLabel(null, null)).toBe("");
  });
});

describe("add summary (US-01.3 AC5)", () => {
  it("counts added, skipped and duplicates", () => {
    expect(addSummary(finished({ added: 3, skipped: 1, duplicates: 2 }))).toBe(
      "3 added, 1 skipped, 2 duplicates",
    );
    expect(addSummary(finished({ added: 1, duplicates: 1 }))).toBe(
      "1 added, 0 skipped, 1 duplicate",
    );
  });

  it("says nothing after a re-scan that found nothing new", () => {
    expect(addSummary(finished({ done: 4 }))).toBeNull();
  });

  it("lists skipped files with the reason and how many more", () => {
    const e = finished({
      skipped: 3,
      skippedFiles: [
        { path: "a.xyz", reason: "unsupported file type" },
        { path: "b.xyz", reason: "unsupported file type" },
      ],
    });
    expect(skippedLines(e)).toEqual([
      "a.xyz — unsupported file type",
      "b.xyz — unsupported file type",
      "and 1 more",
    ]);
  });
});

describe("events of the queue", () => {
  it("re-queries progress on progress changes and documents after a run", () => {
    // Blocked AI jobs come as progress changes too (INV-PR-09).
    expect(staleKeys({ kind: "progressChanged" })).toEqual([["progress"], ["ai_readiness"]]);
    expect(staleKeys(finished())).toContainEqual(["documents"]);
    expect(staleKeys({ kind: "projectChanged" })).toContainEqual(["recent"]);
  });
});

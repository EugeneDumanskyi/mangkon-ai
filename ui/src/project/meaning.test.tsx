import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { MeaningStatus } from "../ipc/bindings/MeaningStatus";
import type { SearchHit } from "../ipc/bindings/SearchHit";
import { meaningCanStart, meaningRowShown, meaningStateLabel, scoreLabel } from "../lib/meaning";
import { MeaningNotReady, MeaningProgressRow } from "./Meaning";
import { SearchPanel, SearchResultRow } from "./SearchPanel";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(null)),
  Channel: Object,
}));

const none = () => undefined;

const info = (over: Partial<MeaningStatus> = {}): MeaningStatus => ({
  state: "running",
  reason: "Embeddings: 40% done",
  itemsDone: 2,
  itemsTotal: 5,
  itemsFailed: 0,
  eta: { kind: "estimating" },
  destination: "Data sent to this machine",
  vectorsModel: null,
  rerunModel: null,
  rerunWith: null,
  waiting: null,
  ...over,
});

const hit: SearchHit = {
  segmentId: 41,
  documentId: 7,
  fileType: "txt",
  path: "/cases/a.txt",
  missing: false,
  location: { kind: "line", line: 3 },
  snippet: [{ text: "Wire transfer to an offshore account", hit: false }],
  hit: { segmentId: 41, start: 0, end: 36 },
};

const withClient = (ui: ReactElement) =>
  renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);

describe("Meaning search, a result (US-06.2 AC1)", () => {
  it("us_06_2_ac1 shows the score and the locator, and no narrowing", () => {
    const html = renderToStaticMarkup(
      <SearchResultRow hit={hit} score={0.8712} active={false} opened={false} onOpen={none} />,
    );
    expect(html).toContain("0.87");
    expect(html).toContain("a.txt");
    expect(html).not.toContain("Only this document");
    expect(scoreLabel(0.5)).toBe("0.50");
  });

  it("a keyword result is as it was", () => {
    const html = renderToStaticMarkup(
      <SearchResultRow
        hit={hit}
        active={false}
        opened={false}
        onOpen={none}
        onOnlyThisDocument={none}
      />,
    );
    expect(html).toContain("Only this document");
    expect(html).not.toContain("data-score");
  });
});

describe("The Search tab (INV-SR-04)", () => {
  it("offers both, keyword first", () => {
    const html = withClient(<SearchPanel opened={null} onOpen={none} />);
    expect(html).toMatch(/aria-pressed="true"[^>]*>Keyword/);
    expect(html).toMatch(/aria-pressed="false"[^>]*>Meaning/);
    expect(html).toContain('aria-label="Search the documents"');
  });
});

describe("Not ready (US-06.2 AC2)", () => {
  const notReady = (over: Partial<MeaningStatus>) =>
    withClient(<MeaningNotReady info={info(over)} />);

  it("us_06_2_ac2 says how far it got, and keyword search still works", () => {
    const html = notReady({});
    expect(html).toContain("Embeddings: 40% done");
    expect(html).toContain("Keyword search works");
    expect(html).not.toContain("Start embeddings");
  });

  it("us_06_2_ac2 no model points to the AI models screen and offers no start", () => {
    const html = notReady({
      state: "blocked",
      reason: "No model allowed for this project",
    });
    expect(html).toContain("No model allowed for this project");
    expect(html).toContain("AI models screen");
    expect(html).not.toContain("Start embeddings");
  });

  it("us_06_2_ac2 a blocked reason is shown as it is", () => {
    const html = notReady({ state: "blocked", reason: "Not enough GPU memory" });
    expect(html).toContain("Not enough GPU memory");
  });

  it("offers a start with its estimate when it is over the limit", () => {
    const html = notReady({
      state: "waitingForStart",
      reason: "Embeddings have not started: about 56 min of work",
      waiting: { windows: 1337115, seconds: 3343 },
    });
    expect(html).toContain("1,337,115 passages");
    expect(html).toContain("about 56 min");
    expect(html).toContain("Start embeddings");
  });

  it("offers a start when nothing has run", () => {
    const html = notReady({ state: "idle", reason: "Embeddings have not run yet" });
    expect(html).toContain("Embeddings have not run yet");
    expect(html).toContain("Start embeddings");
  });

  it("inv_sr_02 offers the re-run when the model changed", () => {
    const html = notReady({
      state: "modelChanged",
      reason: "Vectors come from X. Re-run with Y, or choose X again.",
      vectorsModel: "X",
      rerunWith: "Y",
    });
    expect(html).toContain("Vectors come from X");
    expect(html).toContain("Re-run with Y");
  });

  it("a re-run under way can be cancelled", () => {
    const html = notReady({
      reason: "Re-running embeddings with Y: 10% done. Meaning search comes back when it is done.",
      vectorsModel: "X",
      rerunModel: "Y",
    });
    expect(html).toContain("Cancel re-run");
  });

  it("preparing says so", () => {
    const html = notReady({ state: "preparing", reason: "Preparing meaning search…" });
    expect(html).toContain("Preparing meaning search…");
  });
});

describe("The progress panel's row (M6b)", () => {
  const row = (over: Partial<MeaningStatus>, showPause = true) =>
    withClient(<MeaningProgressRow info={info(over)} showPause={showPause} />);

  it("shows progress, destination, Pause and Stop while it runs", () => {
    const html = row({});
    expect(html).toContain("Meaning search · Embedding · 2 of 5 parts");
    expect(html).toContain("Data sent to this machine");
    expect(html).toContain("Pause");
    expect(html).toContain("Stop");
  });

  it("us_06_2_ac3 shows where data goes on an endpoint", () => {
    const html = row({ destination: "Data sent to 192.168.1.20 (local network)" });
    expect(html).toContain("192.168.1.20 (local network)");
  });

  it("resumes when paused, and leaves Pause to the panel when it has its own", () => {
    expect(row({ state: "paused" })).toContain("Resume");
    expect(row({}, false)).not.toContain("Pause");
  });

  it("counts failed parts", () => {
    expect(row({ state: "ready", reason: null, itemsFailed: 2 })).toContain("2 parts failed");
  });

  it("is quiet when idle or ready", () => {
    expect(row({ state: "idle" })).toBe("");
    expect(row({ state: "ready", reason: null })).toBe("");
    expect(meaningRowShown(info({ state: "ready", itemsFailed: 1 }))).toBe(true);
  });

  it("names every state", () => {
    for (const s of [
      "idle",
      "waitingForStart",
      "running",
      "paused",
      "blocked",
      "modelChanged",
      "preparing",
      "ready",
    ] as const) {
      expect(meaningStateLabel(s).length).toBeGreaterThan(0);
    }
    expect(meaningCanStart(info({ state: "running" }))).toBe(false);
  });
});

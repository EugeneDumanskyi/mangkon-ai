import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { AiExtractionInfo } from "../ipc/bindings/AiExtractionInfo";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import {
  aboutLabel,
  aiMentionNote,
  aiStateLabel,
  aiSummaryNotes,
  confidenceLabel,
} from "../lib/aiExtraction";
import { DEFAULT_FILTER } from "../lib/connections";
import {
  AiProgressRow,
  AiSettingsBody,
  AiStatus,
  ExperimentalLabel,
  FindConfirm,
} from "./AiExtraction";
import { ConnectionSummary, FilterBar } from "./ConnectionsList";
import { ProgressSummary } from "./ProgressPanel";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(null)),
  Channel: Object,
}));

const none = () => undefined;

const info = (over: Partial<AiExtractionInfo> = {}): AiExtractionInfo => ({
  experimental: true,
  state: "running",
  reason: null,
  unitsDone: 3,
  unitsTotal: 10,
  unitsFailed: 0,
  eta: { kind: "estimating" },
  destination: "Data sent to this machine",
  resultsModel: "Qwen 4B",
  rerunModel: null,
  rerunWith: null,
  mentions: 12,
  discarded: 0,
  invalidResponses: 0,
  identifierDropped: 0,
  waiting: null,
  ...over,
});

const withClient = (ui: ReactElement) =>
  renderToStaticMarkup(<QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>);

describe("AI extraction, the job summary (US-10.2 AC2, US-10.1 AC2)", () => {
  it("us_10_2_ac2 shows how many mentions were discarded", () => {
    const html = renderToStaticMarkup(<AiStatus info={info({ discarded: 3 })} />);
    expect(html).toContain("3 mentions discarded (no matching text)");
    expect(aiSummaryNotes(info({ discarded: 1 }))).toEqual([
      "1 mention discarded (no matching text)",
    ]);
  });

  it("us_10_1_ac2 shows how many responses could not be read", () => {
    const html = renderToStaticMarkup(<AiStatus info={info({ invalidResponses: 2 })} />);
    expect(html).toContain("2 responses could not be read");
  });

  it("a clean run has no such notes", () => {
    const html = renderToStaticMarkup(<AiStatus info={info()} />);
    expect(html).not.toContain("discarded");
    expect(html).not.toContain("could not be read");
  });

  it("names the model behind the results and the progress", () => {
    const html = renderToStaticMarkup(<AiStatus info={info()} />);
    expect(html).toContain("12 mentions from Qwen 4B");
    expect(html).toContain("3 of 10 parts");
    expect(html).toContain("Estimating…");
    expect(html).toContain("Data sent to this machine");
  });

  it("Blocked shows the reason, once, with Re-run with Y (decided 2026-11-05)", () => {
    const reason = "These results came from Qwen 4B. New work waits so two models are never mixed.";
    const html = withClient(
      <AiProgressRow
        info={info({ state: "blocked", reason, rerunWith: "Qwen 35B", eta: { kind: "none" } })}
        showPause
      />,
    );
    expect(html).toContain("Blocked");
    expect(html).toContain("New work waits");
    expect(html).toContain("Re-run with Qwen 35B");
    expect(html).not.toContain(">Pause<");
    const panel = renderToStaticMarkup(
      <ProgressSummary
        aiShown
        progress={{
          total: 1,
          done: 1,
          failed: 0,
          queued: 0,
          notProcessed: 0,
          ingesting: false,
          paused: false,
          stage: null,
          runDone: 0,
          runTotal: 0,
          currentFiles: [],
          eta: { kind: "none" },
          blocked: reason,
        }}
      />,
    );
    expect(panel).not.toContain("AI work waits");
    expect(panel).not.toContain("Idle");
  });

  it("a running lane can be paused and stopped from its row", () => {
    const html = withClient(<AiProgressRow info={info()} showPause />);
    expect(html).toContain("Pause");
    expect(html).toContain("Stop");
    const whileReading = withClient(<AiProgressRow info={info()} showPause={false} />);
    expect(whileReading).not.toContain("Pause");
  });

  it("nothing shows before anything has run", () => {
    expect(withClient(<AiProgressRow info={info({ state: "idle" })} showPause />)).toBe("");
  });

  it("gap A3: over the limit it asks, with the estimate", () => {
    const html = withClient(
      <AiProgressRow
        info={info({
          state: "waitingForPick",
          resultsModel: null,
          waiting: { units: 2_000, seconds: 1_440 },
        })}
        showPause
      />,
    );
    expect(html).toContain("2,000 parts of text wait, about 24 min");
    expect(html).toContain("Find people and organizations");
  });

  it("gap A3: the confirm names how much and how long", () => {
    const all = renderToStaticMarkup(
      <FindConfirm
        estimate={{ units: 500, seconds: 360 }}
        files={0}
        running={false}
        onRun={none}
        onClose={none}
      />,
    );
    expect(all).toContain("in all files");
    expect(all).toContain("500 parts of text, about 6 min");
    const picked = renderToStaticMarkup(
      <FindConfirm
        estimate={{ units: 1, seconds: 1 }}
        files={2}
        running={false}
        onRun={none}
        onClose={none}
      />,
    );
    expect(picked).toContain("in 2 files");
    expect(picked).toContain("less than a minute");
    const nothing = renderToStaticMarkup(
      <FindConfirm
        estimate={{ units: 0, seconds: 0 }}
        files={0}
        running={false}
        onRun={none}
        onClose={none}
      />,
    );
    expect(nothing).toContain("Nothing new to read");
    expect(nothing).not.toContain("Find them");
  });
});

describe("Experimental label (US-10.3)", () => {
  it("us_10_3_ac1 follows the build's flag", () => {
    expect(renderToStaticMarkup(<ExperimentalLabel on />)).toContain("Experimental");
    expect(renderToStaticMarkup(<ExperimentalLabel on={false} />)).toBe("");
  });

  it("us_10_3_ac1 is on the row, in Settings and on a mention, or on none of them", () => {
    expect(renderToStaticMarkup(<AiStatus info={info()} />)).toContain("Experimental");
    const off = info({ experimental: false });
    expect(renderToStaticMarkup(<AiStatus info={off} />)).not.toContain("Experimental");
    expect(withClient(<AiSettingsBody info={info()} />)).toContain("Experimental");
    expect(withClient(<AiSettingsBody info={off} />)).not.toContain("Experimental");
  });
});

describe("re-run (US-10.4)", () => {
  it("us_10_4_ac1 Settings shows the model behind the results and offers the other", () => {
    const html = withClient(<AiSettingsBody info={info({ rerunWith: "Qwen 35B" })} />);
    expect(html).toContain("The results come from");
    expect(html).toContain("Qwen 4B");
    expect(html).toContain("Re-run with Qwen 35B");
  });

  it("under way, it says the results stay and can be cancelled", () => {
    const html = withClient(<AiSettingsBody info={info({ rerunModel: "Qwen 35B" })} />);
    expect(html).toContain("Re-running with");
    expect(html).toContain("The results stay until it is done");
    expect(html).toContain("Cancel re-run");
  });

  it("while the switch runs it says so, and offers nothing else", () => {
    const html = withClient(
      <AiSettingsBody info={info({ state: "applying", rerunWith: "Qwen 35B" })} />,
    );
    expect(html).toContain("Applying new results…");
    expect(html).not.toContain("Re-run with");
    expect(aiStateLabel("applying")).toBe("Applying new results…");
  });
});

describe("low confidence (US-10.1 AC3)", () => {
  const person = (over: Partial<ConnectionItem>): ConnectionItem => ({
    key: "person:corvin hale",
    kind: "person",
    value: "Corvin Hale",
    label: null,
    fromTable: false,
    sourceCount: 2,
    fileTypeCount: 1,
    mentionCount: 2,
    fileTypes: ["txt"],
    lowConfidence: false,
    lowOnly: false,
    ignored: false,
    linkedIdentifiers: [],
    linkedTotal: 0,
    ...over,
  });

  it("us_10_1_ac3 the filter has the toggle, off by default", () => {
    const html = renderToStaticMarkup(
      <FilterBar filter={DEFAULT_FILTER} fileTypes={[]} ignored={0} onChange={none} />,
    );
    expect(DEFAULT_FILTER.includeLow).toBe(false);
    expect(html).toContain("Show low confidence");
    expect(html).not.toContain("Clear filters");
    const on = renderToStaticMarkup(
      <FilterBar
        filter={{ ...DEFAULT_FILTER, includeLow: true }}
        fileTypes={[]}
        ignored={0}
        onChange={none}
      />,
    );
    expect(on).toContain("Clear filters");
  });

  it("us_10_1_ac3 a row only low mentions make says so", () => {
    expect(
      renderToStaticMarkup(<ConnectionSummary row={person({ lowOnly: true })} selected={false} />),
    ).toContain("Low confidence");
    expect(
      renderToStaticMarkup(<ConnectionSummary row={person({})} selected={false} />),
    ).not.toContain("Low confidence");
  });

  it("us_10_1_ac1 an AI mention shows its confidence and model", () => {
    const o = {
      aiModel: "Qwen 4B",
      confidence: 0.87,
    } as Occurrence;
    expect(aiMentionNote(o, true)).toBe(" · 87% · Qwen 4B · Experimental");
    expect(aiMentionNote(o, false)).toBe(" · 87% · Qwen 4B");
    expect(aiMentionNote({ aiModel: null, confidence: 1 } as Occurrence, true)).toBe("");
    expect(confidenceLabel(0.5)).toBe("50%");
  });
});

describe("estimates", () => {
  it("reads in plain time", () => {
    expect(aboutLabel(30)).toBe("less than a minute");
    expect(aboutLabel(1_200)).toBe("about 20 min");
    expect(aboutLabel(7_500)).toBe("about 2 h 5 min");
    expect(aboutLabel(7_200)).toBe("about 2 h");
  });
});

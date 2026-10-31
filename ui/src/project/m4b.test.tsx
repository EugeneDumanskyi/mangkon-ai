import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { DocumentItem } from "../ipc/bindings/DocumentItem";
import type { Progress } from "../ipc/bindings/Progress";
import { reprocessQuestion } from "../lib/forms";
import { kindLabel, kindWarning } from "../lib/labels";
import { ConnectionSummary } from "./ConnectionsList";
import { DocumentRowView } from "./DocumentsList";
import { ProgressSummary } from "./ProgressPanel";
import { ReprocessPrompt, SettingsForm } from "./SettingsPanel";
import { CardFlags } from "./SourcePane";

const none = () => undefined;

const doc = (over: Partial<DocumentItem>): DocumentItem => ({
  refId: 1,
  contentId: 1,
  path: "/src/invoice.txt",
  fileType: "txt",
  size: 120,
  units: null,
  unit: null,
  status: "done",
  warnings: [],
  failureReason: null,
  failureDetail: null,
  otherPaths: 0,
  cardLikeCount: 0,
  mappingState: null,
  ...over,
});

const connection = (over: Partial<ConnectionItem>): ConnectionItem => ({
  key: "phone:+442079460318",
  kind: "phone",
  value: "+442079460318",
  sourceCount: 2,
  fileTypeCount: 1,
  mentionCount: 3,
  lowConfidence: false,
  label: null,
  fromTable: false,
  ...over,
});

describe("card-like numbers (US-04.6, NFR-SEC-14)", () => {
  it("badges a document that contains one, and only that one", () => {
    const row = (n: number) =>
      renderToStaticMarkup(
        <DocumentRowView
          row={doc({ cardLikeCount: n })}
          checked={false}
          onCheck={none}
          onDetails={none}
        />,
      );
    expect(row(2)).toContain("Contains card-like number");
    expect(row(0)).not.toContain("card-like");
  });

  it("lists them masked with their places in the viewer", () => {
    const html = renderToStaticMarkup(
      <CardFlags
        flags={[
          { masked: "**** **** **** 1111", location: { kind: "line", line: 3 } },
          {
            masked: "**** **** **** 0004",
            location: { kind: "cell", row: 1, columnName: "card", columnIndex: 1 },
          },
        ]}
      />,
    );
    expect(html).toContain("**** **** **** 1111");
    expect(html).toContain("(line 3)");
    expect(html).toContain("(row 1, column card)");
    // No digits other than the last four and the places.
    expect(html.replace(/<[^>]*>/g, "").replace(/\D/g, "")).toBe("1111300041");
    expect(renderToStaticMarkup(<CardFlags flags={[]} />)).toBe("");
  });
});

describe("identifier types (US-04.2 – US-04.5)", () => {
  it("names each type", () => {
    expect(kindLabel("iban")).toBe("IBAN");
    expect(kindLabel("btc")).toBe("Bitcoin address");
    expect(kindLabel("phone_unnormalized")).toBe("phone");
    expect(kindLabel("future_type")).toBe("future_type");
  });

  it("warns on a phone kept as digits without a region (US-04.2 AC3)", () => {
    expect(kindWarning("phone")).toBeNull();
    const html = renderToStaticMarkup(
      <ConnectionSummary
        row={connection({ kind: "phone_unnormalized", value: "02079460318" })}
        selected={false}
      />,
    );
    expect(html).toContain("No region — exact digits only");
    expect(html).toContain("phone · 2 sources");
    const normal = renderToStaticMarkup(<ConnectionSummary row={connection({})} selected />);
    expect(normal).not.toContain("No region");
  });
});

describe("settings (US-04.2 AC2, US-04.3 AC3)", () => {
  it("asks to find phone numbers again after a region change, reading no file", () => {
    const saved = { reprocessOcr: false, rerunIdentifiers: true };
    expect(reprocessQuestion(saved)).toBe(
      "Saved. The phone region changed: find phone numbers again now?",
    );
    const html = renderToStaticMarkup(
      <ReprocessPrompt saved={saved} busy={false} onReprocess={none} onLater={none} />,
    );
    expect(html).toContain("no file is read again");
    expect(html).toContain("Re-process");
  });

  it("names both changes, and asks nothing when nothing changed", () => {
    expect(reprocessQuestion({ reprocessOcr: true, rerunIdentifiers: true })).toBe(
      "Saved. The OCR languages and the phone region changed: re-process the affected files now?",
    );
    expect(reprocessQuestion({ reprocessOcr: false, rerunIdentifiers: false })).toBeNull();
    const html = renderToStaticMarkup(
      <ReprocessPrompt
        saved={{ reprocessOcr: false, rerunIdentifiers: false }}
        busy={false}
        onReprocess={none}
        onLater={none}
      />,
    );
    expect(html).toBe("");
  });

  it("says the region and the stop list now take effect", () => {
    const html = renderToStaticMarkup(
      <SettingsForm
        settings={{ ocrLanguages: ["eng"], phoneRegion: null, domainStopList: ["gmail.com"] }}
        installed={["eng"]}
        saving={false}
        onSave={none}
      />,
    );
    expect(html).not.toContain("later version");
    expect(html).toContain("only match the same digits");
    expect(html).toContain("Changes apply at once");
  });
});

describe("progress of the identifier re-run", () => {
  it("shows the re-run with its files and no stage letter", () => {
    const progress: Progress = {
      total: 40,
      done: 40,
      failed: 0,
      queued: 0,
      notProcessed: 0,
      ingesting: true,
      paused: false,
      stage: "identifiers",
      runDone: 12,
      runTotal: 40,
      currentFiles: [],
      eta: { kind: "none" },
    };
    const html = renderToStaticMarkup(<ProgressSummary progress={progress} />);
    expect(html).toContain("Finding identifiers again in stored text");
    expect(html).toContain("12 of 40 files");
    expect(html).not.toContain("Stage");
  });
});

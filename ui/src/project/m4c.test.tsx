import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { DocumentItem } from "../ipc/bindings/DocumentItem";
import type { MappingView } from "../ipc/bindings/MappingView";
import { columnTypeLabel, mappingStateLabel } from "../lib/labels";
import { ConnectionSummary } from "./ConnectionsList";
import { PatternEditor, TOO_BROAD } from "./CustomPatterns";
import { DocumentRowView } from "./DocumentsList";
import { MappingForm } from "./MappingScreen";

const none = () => undefined;

const doc = (over: Partial<DocumentItem>): DocumentItem => ({
  refId: 1,
  contentId: 1,
  path: "/src/payments.csv",
  fileType: "csv",
  size: 120,
  units: 10,
  unit: "rows",
  status: "done",
  warnings: [],
  failureReason: null,
  failureDetail: null,
  otherPaths: 0,
  cardLikeCount: 0,
  mappingState: "suggested",
  ...over,
});

const connection = (over: Partial<ConnectionItem>): ConnectionItem => ({
  key: "org:brightwater logistics",
  kind: "org",
  value: "Brightwater Logistics Ltd",
  label: null,
  fromTable: true,
  sourceCount: 3,
  fileTypeCount: 2,
  mentionCount: 5,
  lowConfidence: false,
  lowOnly: false,
  fileTypes: [],
  ignored: false,
  linkedIdentifiers: [],
  linkedTotal: 0,
  ...over,
});

const view: MappingView = {
  refId: 7,
  path: "/src/payments.csv",
  fileType: "csv",
  state: "suggested",
  columnCount: 3,
  columns: [
    {
      key: "Supplier",
      header: "Supplier",
      samples: ["Brightwater Logistics Ltd", "Harbour Freight"],
      suggested: "org",
      assigned: null,
    },
    {
      key: "Amount",
      header: "Amount",
      samples: ["12.00"],
      suggested: "ignore",
      assigned: null,
    },
  ],
  reuse: [{ refId: 3, path: "/src/payments-2023.csv" }],
};

describe("custom patterns (US-04.7)", () => {
  it("names a custom connection after its pattern", () => {
    const html = renderToStaticMarkup(
      <ConnectionSummary
        row={connection({
          key: "custom:tender_ref:VPA-TND-2024-017",
          kind: "custom",
          value: "VPA-TND-2024-017",
          label: "tender_ref",
          fromTable: false,
        })}
        selected={false}
      />,
    );
    expect(html).toContain("tender_ref ·");
    expect(html).toContain("VPA-TND-2024-017");
    expect(html).not.toContain("Name (from table)");
  });

  it("locks the name of a saved pattern and shows a save error", () => {
    const html = renderToStaticMarkup(
      <PatternEditor
        editing={{ name: "tender_ref", expression: "VPA-\\d+", matchCount: 3, tooBroad: false }}
        saving={false}
        error="A pattern named tender_ref already exists"
        onSave={none}
        onCancel={none}
      />,
    );
    expect(html).toMatch(/<input[^>]*disabled=""[^>]*>/);
    expect(html).toContain('value="tender_ref"');
    expect(html).toContain("already exists");
  });

  it("has the too-broad warning text of AC4", () => {
    expect(TOO_BROAD).toBe("This pattern matches a lot. It may be too broad.");
  });
});

describe("column mapping (US-05.1, US-05.2)", () => {
  it("shows the mapping state and Map columns on table files only", () => {
    const csv = renderToStaticMarkup(
      <DocumentRowView
        row={doc({})}
        checked={false}
        onCheck={none}
        onDetails={none}
        onMap={none}
      />,
    );
    expect(csv).toContain("Columns: Suggested");
    expect(csv).toContain("Map columns");
    const txt = renderToStaticMarkup(
      <DocumentRowView
        row={doc({ fileType: "txt", mappingState: null })}
        checked={false}
        onCheck={none}
        onDetails={none}
        onMap={none}
      />,
    );
    expect(txt).not.toContain("Columns:");
    expect(txt).not.toContain("Map columns");
    expect(mappingStateLabel("needs_review")).toBe("Needs review");
  });

  it("lists header, samples and a suggested type per column, with reuse and Confirm", () => {
    const html = renderToStaticMarkup(
      <MappingForm view={view} busy={false} onConfirm={none} onReuse={none} />,
    );
    expect(html).toContain("Supplier");
    expect(html).toContain("Brightwater Logistics Ltd");
    expect(html).toContain("Suggested");
    expect(html).toContain("Use the same mapping as payments-2023.csv");
    expect(html).toContain("Confirm");
    expect(html).toContain("1 more columns are not shown");
    expect(html).toMatch(/<option value="org" selected="">Organization name<\/option>/);
    expect(columnTypeLabel("person")).toBe("Person name");
  });

  it("drops the Suggested pill once a type is confirmed", () => {
    const confirmed: MappingView = {
      ...view,
      state: "confirmed",
      columnCount: 2,
      columns: view.columns.map((c) => ({ ...c, assigned: c.suggested })),
      reuse: [],
    };
    const html = renderToStaticMarkup(
      <MappingForm view={confirmed} busy={false} onConfirm={none} onReuse={none} />,
    );
    expect(html).not.toContain("Suggested");
  });
});

describe("names from tables (US-05.3 AC2)", () => {
  it("shows the display name, the entity type and the Name (from table) pill", () => {
    const html = renderToStaticMarkup(<ConnectionSummary row={connection({})} selected={false} />);
    expect(html).toContain("Brightwater Logistics Ltd");
    expect(html).toContain("Name (from table)");
    expect(html).toContain("Organization ·");
  });
});

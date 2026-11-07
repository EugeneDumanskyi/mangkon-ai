import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DecisionItem } from "../ipc/bindings/DecisionItem";
import {
  decisionChange,
  decisionKindLabel,
  decisionStatusLabel,
  decisionTime,
} from "../lib/labels";
import { DecisionList, DecisionRow } from "./DecisionList";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;

const item = (over: Partial<DecisionItem> = {}): DecisionItem => ({
  seq: 1,
  kind: "rename",
  anchor: "person:corvin hale",
  before: "Corvin Hale",
  after: "C. Hale",
  decidedAt: "2026-11-07T10:15:00Z",
  status: "active",
  entityKey: "person:corvin hale",
  moved: null,
  ...over,
});

// One row per kind the log can hold, with the words each must show.
const KINDS: [Partial<DecisionItem>, string, string][] = [
  [
    {
      kind: "ignore_identifier",
      anchor: "email:a@x.example",
      before: "shown",
      after: "ignored",
    },
    "Ignored",
    "a@x.example: shown → ignored",
  ],
  [
    {
      kind: "unignore_identifier",
      anchor: "email:a@x.example",
      before: "ignored",
      after: "shown",
    },
    "Un-ignored",
    "a@x.example: ignored → shown",
  ],
  [
    { kind: "ignore_entity", before: "shown", after: "ignored" },
    "Ignored",
    "corvin hale: shown → ignored",
  ],
  [
    { kind: "unignore_entity", before: "ignored", after: "shown" },
    "Un-ignored",
    "corvin hale: ignored → shown",
  ],
  [
    { kind: "confirm_mapping", before: null, after: '{"columns":[]}' },
    "Confirmed columns",
    "Columns of a table file",
  ],
  [
    {
      kind: "accept_merge",
      anchor: "person:c hale|person:corvin hale",
      before: "C. Hale / Corvin Hale",
      after: "Corvin Hale",
    },
    "Merged",
    "C. Hale / Corvin Hale → Corvin Hale",
  ],
  [
    {
      kind: "reject_merge",
      anchor: "person:c hale|person:corvin hale",
      before: "proposed",
      after: "rejected",
    },
    "Rejected merge",
    "c hale / corvin hale: proposed → rejected",
  ],
  [
    {
      kind: "reject_merge",
      anchor: "person:c hale|person:corvin hale",
      before: "merged",
      after: "rejected",
    },
    "Undid merge",
    "c hale / corvin hale: merged → rejected",
  ],
  [
    { kind: "split", anchor: "split:ab", before: "Corvin Hale", after: null, moved: 2 },
    "Split",
    "Corvin Hale → 2 mentions moved",
  ],
  [
    { kind: "undo_split", anchor: "split:ab", before: "split", after: "put back" },
    "Put back",
    "Its mentions went back",
  ],
  [{}, "Renamed", "Corvin Hale → C. Hale"],
  [
    { kind: "change_type", before: "person", after: "org" },
    "Changed type",
    "Person → Organization",
  ],
  [{ kind: "set_note", before: null, after: "Seen" }, "Note", "Seen"],
  [{ kind: "set_note", before: "Seen", after: "" }, "Note", "Note cleared"],
  [
    {
      kind: "remove_alias",
      anchor: "person:corvin hale#alias:Mr Hale",
      before: "alias",
      after: "removed",
    },
    "Removed alias",
    "Mr Hale: alias → removed",
  ],
  [
    {
      kind: "restore_alias",
      anchor: "person:corvin hale#alias:Mr Hale",
      before: "removed",
      after: "alias",
    },
    "Restored alias",
    "Mr Hale: removed → alias",
  ],
];

describe("the decision list's words (US-11.5 AC2)", () => {
  it.each(KINDS)("%j", (over, kind, change) => {
    const d = item(over);
    expect(decisionKindLabel(d)).toBe(kind);
    expect(decisionChange(d)).toBe(change);
  });

  it("names every kind the log has", () => {
    const named = new Set(KINDS.map(([over]) => over.kind ?? "rename"));
    expect(named.size).toBe(14);
  });

  it("never shows a split's keys or a mapping's JSON", () => {
    const split = decisionChange(item({ kind: "split", after: null, moved: 1 }));
    expect(split).toBe("Corvin Hale → 1 mention moved");
    const mapping = decisionChange(item({ kind: "confirm_mapping", after: '{"a":1}' }));
    expect(mapping).not.toContain("{");
  });

  it("says whether it still applies (domain §7.5)", () => {
    expect(decisionStatusLabel(item())).toBe("Applies");
    expect(decisionStatusLabel(item({ status: "superseded" }))).toBe(
      "Replaced by a newer decision",
    );
    expect(decisionStatusLabel(item({ status: "orphaned" }))).toBe(
      "No longer applies — the source is gone or changed",
    );
    expect(decisionStatusLabel(item({ kind: "confirm_mapping", status: null }))).toBe("");
  });

  it("does not say a merge that holds is gone (M7a open point)", () => {
    const text = decisionStatusLabel(item({ kind: "accept_merge", status: "orphaned" }));
    expect(text).toBe(
      "One of these names has no mentions now. It applies again when they come back.",
    );
  });

  it("shows local time, or the stored text when it cannot read it", () => {
    expect(decisionTime("2026-11-07T10:15:00Z")).toBe(
      new Date(Date.UTC(2026, 10, 7, 10, 15)).toLocaleString(),
    );
    expect(decisionTime("not a time")).toBe("not a time");
  });
});

describe("a decision row", () => {
  it("has an Open button when it is on an entity", () => {
    const html = renderToStaticMarkup(<DecisionRow decision={item()} onOpenEntity={none} />);
    expect(html).toContain("Renamed");
    expect(html).toContain("Corvin Hale → C. Hale");
    expect(html).toContain(">Open<");
    expect(html).toContain("Applies");
  });

  it("has none when it is on an identifier or a file", () => {
    const html = renderToStaticMarkup(
      <DecisionRow
        decision={item({ kind: "ignore_identifier", entityKey: null })}
        onOpenEntity={none}
      />,
    );
    expect(html).not.toContain(">Open<");
  });

  it("the empty list says so", () => {
    const client = new QueryClient();
    client.setQueryData(["decisions"], {
      pages: [{ rows: [], next: null }],
      pageParams: [null],
    });
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <DecisionList onOpenEntity={none} />
      </QueryClientProvider>,
    );
    expect(html).toContain("No decisions yet.");
    expect(html).toContain("Your decisions");
  });
});

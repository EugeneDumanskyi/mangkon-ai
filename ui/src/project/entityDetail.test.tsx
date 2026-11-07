import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import { firstOf } from "../lib/review";
import { type EntityActions, EntityPage } from "./EntityDetail";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;
const actions: EntityActions = {
  rename: none,
  setType: none,
  setNote: none,
  removeAlias: none,
  restoreAlias: none,
  undoMerge: none,
  split: none,
  putBack: none,
};

const detail = (over: Partial<EntityDetail> = {}): EntityDetail => ({
  key: "person:corvin hale",
  name: "Corvin Hale",
  entityType: "person",
  note: "Seen at the port",
  fromTable: false,
  mentionCount: 7,
  aliases: [{ name: "Mr Hale", mentions: 2 }],
  removedAliases: [{ name: "C. H.", mentions: 1 }],
  linkedIdentifiers: [{ key: "email:c@x.example", kind: "email", value: "c@x.example" }],
  linkedTotal: 3,
  documents: [
    { documentId: 4, fileType: "txt", path: "/case/a.txt", mentions: 5 },
    { documentId: 5, fileType: "eml", path: null, mentions: 2 },
  ],
  documentsTotal: 2,
  evidenceTotal: 2,
  merges: [],
  splits: [],
  splitFrom: [],
  isSplit: false,
  pendingProposals: 0,
  ...over,
});

const mention = (id: number, over: Partial<Occurrence> = {}): Occurrence => ({
  mentionId: id,
  key: "person:corvin hale",
  fileType: "txt",
  path: "/case/a.txt",
  location: { kind: "line", line: 2 },
  raw: "Corvin Hale",
  lowConfidence: false,
  confidence: 0.9,
  aiModel: "Qwen 4B",
  documentId: 4,
  snippet: [{ text: "Corvin Hale", hit: true }],
  ...over,
});

function page(d: EntityDetail, evidence: Occurrence[] = [mention(-1), mention(-2)]): string {
  return renderToStaticMarkup(
    <EntityPage
      detail={d}
      evidence={evidence}
      selected={null}
      includeLow={false}
      experimental={false}
      busy={false}
      actions={actions}
      onMention={none}
      onDocument={none}
      onOpenEntity={none}
      onReview={none}
      onLow={none}
    />,
  );
}

describe("an entity's page (US-12.1 AC1)", () => {
  it("shows name, type, aliases, identifiers, documents with counts, and evidence", () => {
    const html = page(detail());
    expect(html).toContain('value="Corvin Hale"');
    expect(html).toContain(">Organization<");
    expect(html).toContain("Mr Hale");
    expect(html).toContain("c@x.example");
    expect(html).toContain("2 more");
    expect(html).toContain("Appears in 2 documents");
    expect(html).toContain("a.txt");
    expect(html).toContain("5 mentions");
    expect(html).toContain("(file no longer in the project)");
    expect(html).toContain("Evidence (2)");
    expect(html).toMatch(/<mark data-hit="true"[^>]*>Corvin Hale<\/mark>/);
    expect(html).toContain("Show low confidence");
  });

  it("makes each piece of evidence one button of its own (US-12.1 AC2)", () => {
    const html = page(detail());
    expect(html.match(/data-mention-id=/g)).toHaveLength(2);
    expect(html).toContain("line 2");
  });

  it("says when a list stops before the end (the 1,000 cap)", () => {
    expect(firstOf(1_000, 1_000, "mentions")).toBeNull();
    expect(firstOf(1_000, 4_200, "mentions")).toBe("Showing the first 1,000 of 4,200 mentions");
    const html = page(detail({ evidenceTotal: 1_200, documentsTotal: 1_001 }));
    expect(html).toContain("Showing the first 2 of 1,200 mentions");
    expect(html).toContain("Showing the first 2 of 1,001 documents");
  });
});

describe("what evidence says (US-07.7 AC3)", () => {
  it("gives a model's name and confidence", () => {
    expect(page(detail())).toContain("line 2 · 90% · Qwen 4B");
  });

  it("says a table's mention came from a table, with no confidence", () => {
    const html = page(detail(), [mention(-1, { aiModel: null, confidence: 1 })]);
    expect(html).toContain("From a table");
    expect(html).not.toContain("100%");
  });
});

describe("editing (US-12.2)", () => {
  it("offers rename, type, note, removing an alias and restoring a removed one", () => {
    const html = page(detail());
    expect(html).toContain(">Rename<");
    expect(html).toContain("Seen at the port");
    expect(html).toContain(">Save note<");
    expect(html).toContain('aria-label="Remove alias Mr Hale"');
    expect(html).toContain('aria-label="Restore alias C. H."');
  });
});

describe("undo and split (US-11.4)", () => {
  it("offers Undo merge per merged pair", () => {
    const html = page(
      detail({
        merges: [
          { a: "person:a", b: "person:b", aName: "A", bName: "B" },
          { a: "person:b", b: "person:c", aName: "B", bName: "C" },
        ],
      }),
    );
    expect(html).toContain('aria-label="Undo merge of A and B"');
    expect(html).toContain('aria-label="Undo merge of B and C"');
  });

  it("has no Undo merge on an unmerged entity", () => {
    expect(page(detail())).not.toContain("Undo merge");
  });

  it("a checkbox per mention, and a Split button off until one is picked", () => {
    const html = page(detail());
    expect(html.match(/aria-label="Pick /g)).toHaveLength(2);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Split 0 into a new entity/);
  });

  it("lists what was split out, so it stays reachable", () => {
    const html = page(detail({ splits: [{ key: "split:ab", name: "Corvin Hale", mentions: 2 }] }));
    expect(html).toContain("Split out of this one");
    expect(html).toContain("2 mentions");
  });

  it("offers Put back on a split entity, naming where it came from", () => {
    const html = page(
      detail({
        isSplit: true,
        splitFrom: [{ key: "person:corvin hale", name: "Corvin", mentions: 5 }],
      }),
    );
    expect(html).toContain(">Put back<");
    expect(html).toContain("From Corvin");
  });

  it("offers the review queue when proposals wait (US-07.7 AC4)", () => {
    expect(page(detail({ pendingProposals: 2 }))).toContain("2 merge proposals to review");
    expect(page(detail())).not.toContain("to review");
  });
});

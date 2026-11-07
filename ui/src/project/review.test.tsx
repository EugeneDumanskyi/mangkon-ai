import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import type { ProposalItem } from "../ipc/bindings/ProposalItem";
import {
  type ReviewKey,
  clampIndex,
  defaultChoice,
  editedChoice,
  knownNames,
  queueUpdated,
  reviewAction,
  scoreText,
} from "../lib/review";
import { MergeEditForm, ProposalHead, ProposalSide, ReviewQueue } from "./ReviewQueue";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;

const key = (k: string, over: Partial<ReviewKey> = {}): ReviewKey => ({
  key: k,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  repeat: false,
  targetTag: "BODY",
  targetEditable: false,
  ...over,
});

const proposal = (over: Partial<ProposalItem> = {}): ProposalItem => ({
  a: "person:c hale",
  b: "person:corvin hale",
  leftKey: "person:c hale",
  rightKey: "person:corvin hale",
  leftName: "C. Hale",
  rightName: "Corvin Hale",
  score: 0.8612,
  reasons: ["Similar names (0.86)", "Same phone +4420 in 2 documents"],
  ...over,
});

export const detail = (over: Partial<EntityDetail> = {}): EntityDetail => ({
  key: "person:corvin hale",
  name: "Corvin Hale",
  entityType: "person",
  note: null,
  fromTable: false,
  mentionCount: 7,
  aliases: [{ name: "Mr Hale", mentions: 2 }],
  removedAliases: [],
  linkedIdentifiers: [{ key: "email:c@x.example", kind: "email", value: "c@x.example" }],
  linkedTotal: 1,
  documents: [],
  documentsTotal: 3,
  evidenceTotal: 7,
  merges: [],
  splits: [],
  splitFrom: [],
  isSplit: false,
  pendingProposals: 0,
  ...over,
});

export const mention = (id: number): Occurrence => ({
  mentionId: id,
  key: "person:corvin hale",
  fileType: "txt",
  path: "/case/a.txt",
  location: { kind: "line", line: 2 },
  raw: "Corvin Hale",
  lowConfidence: false,
  confidence: 0.9,
  aiModel: "Qwen 4B",
  documentId: 1,
  snippet: [
    { text: "Paid by ", hit: false },
    { text: "Corvin Hale", hit: true },
  ],
});

describe("the queue's keys (US-11.3 AC5)", () => {
  it("maps A, R, E, J and K in either case", () => {
    expect(reviewAction(key("a"))).toBe("accept");
    expect(reviewAction(key("r"))).toBe("reject");
    expect(reviewAction(key("e"))).toBe("edit");
    expect(reviewAction(key("j"))).toBe("next");
    expect(reviewAction(key("k"))).toBe("previous");
    expect(reviewAction(key("A"))).toBe("accept");
    expect(reviewAction(key("x"))).toBeNull();
    expect(reviewAction(key("Enter"))).toBeNull();
  });

  it("leaves typing in a field alone", () => {
    for (const targetTag of ["INPUT", "TEXTAREA", "SELECT"]) {
      expect(reviewAction(key("a", { targetTag }))).toBeNull();
    }
    expect(reviewAction(key("a", { targetEditable: true }))).toBeNull();
    expect(reviewAction(key("a", { targetTag: "BUTTON" }))).toBe("accept");
  });

  it("leaves keys with a modifier alone", () => {
    for (const mod of ["ctrlKey", "metaKey", "altKey", "shiftKey"] as const) {
      expect(reviewAction(key("a", { [mod]: true }))).toBeNull();
    }
  });

  it("does not decide over and over while a key is held, but moves", () => {
    for (const k of ["a", "r", "e"]) expect(reviewAction(key(k, { repeat: true }))).toBeNull();
    expect(reviewAction(key("j", { repeat: true }))).toBe("next");
    expect(reviewAction(key("k", { repeat: true }))).toBe("previous");
  });
});

describe("the place in the queue after a decision", () => {
  it("takes the same index, or the last when it was the last", () => {
    expect(clampIndex(3, 10)).toBe(3);
    expect(clampIndex(9, 9)).toBe(8);
    expect(clampIndex(-1, 5)).toBe(0);
    expect(clampIndex(0, 0)).toBe(0);
  });
});

describe("a queue that holds still", () => {
  const p = proposal();
  it("is behind when the count or the first row moved, not otherwise", () => {
    const shown = { total: 3, head: p };
    expect(queueUpdated({ total: 3, head: p }, shown)).toBe(false);
    expect(queueUpdated({ total: 4, head: p }, shown)).toBe(true);
    expect(queueUpdated({ total: 3, head: proposal({ b: "person:other" }) }, shown)).toBe(true);
  });
});

describe("Edit (US-11.3 AC4)", () => {
  const left = detail({
    name: "C. Hale",
    mentionCount: 2,
    aliases: [{ name: "Hale", mentions: 1 }],
  });
  const right = detail({ name: "Corvin Hale", mentionCount: 7, aliases: [] });

  it("offers both sides' names, each once", () => {
    expect(knownNames(left, right)).toEqual(["C. Hale", "Hale", "Corvin Hale"]);
    expect(knownNames(left, detail({ name: "C. Hale", aliases: [] }))).toEqual(["C. Hale", "Hale"]);
    expect(knownNames(null, null)).toEqual([]);
  });

  it("starts from the more frequent name (US-11.3 AC2)", () => {
    expect(defaultChoice(left, right).name).toBe("Corvin Hale");
    expect(defaultChoice(right, left).name).toBe("Corvin Hale");
  });

  it("sends only what the user changed", () => {
    const d = { name: "Corvin Hale", entityType: "person" };
    expect(editedChoice(d, d)).toEqual({ name: null, entityType: null });
    expect(editedChoice({ name: " New ", entityType: "person" }, d)).toEqual({
      name: "New",
      entityType: null,
    });
    expect(editedChoice({ name: "Corvin Hale", entityType: "org" }, d)).toEqual({
      name: null,
      entityType: "org",
    });
  });

  it("has the name, the known names, the type, Accept and Cancel", () => {
    const html = renderToStaticMarkup(
      <MergeEditForm left={left} right={right} busy={false} onAccept={none} onCancel={none} />,
    );
    expect(html).toContain('value="Corvin Hale"');
    expect(html).toContain(">C. Hale<");
    expect(html).toContain(">Organization<");
    expect(html).toContain(">Accept<");
    expect(html).toContain(">Cancel<");
  });
});

describe("a proposal on screen (US-11.2 AC1, US-11.3 AC1)", () => {
  it("shows its score and plain reasons, with a real button per key", () => {
    const html = renderToStaticMarkup(
      <ProposalHead proposal={proposal()} busy={false} onAction={none} />,
    );
    expect(scoreText(0.8612)).toBe("0.86");
    expect(html).toContain("Score 0.86");
    expect(html).toContain("Similar names (0.86)");
    expect(html).toContain("Same phone +4420 in 2 documents");
    for (const label of ["Accept", "Reject", "Edit", "Previous", "Next"]) {
      expect(html).toContain(`${label} <kbd`);
    }
    for (const k of ["a", "r", "e", "j", "k"]) {
      expect(html).toContain(`aria-keyshortcuts="${k}"`);
    }
  });

  it("shows a side's name, aliases, identifiers and evidence, each snippet one button", () => {
    const html = renderToStaticMarkup(
      <ProposalSide
        label="First entity"
        name="Corvin Hale"
        detail={detail()}
        evidence={[mention(-1), mention(-2)]}
        selected={-2}
        onMention={none}
        onDetails={none}
      />,
    );
    for (const text of ["Corvin Hale", "Mr Hale", "c@x.example", "Details"]) {
      expect(html).toContain(text);
    }
    expect(html.match(/data-mention-id=/g)).toHaveLength(2);
    expect(html).toMatch(/<mark data-hit="true"[^>]*>Corvin Hale<\/mark>/);
    expect(html).toContain('aria-pressed="true"');
  });

  it("counts identifiers past those it lists", () => {
    const many = Array.from({ length: 12 }, (_, i) => ({
      key: `email:${String(i)}@x.example`,
      kind: "email",
      value: `${String(i)}@x.example`,
    }));
    const html = renderToStaticMarkup(
      <ProposalSide
        label="First entity"
        name="Corvin Hale"
        detail={detail({ linkedIdentifiers: many, linkedTotal: 250 })}
        evidence={[]}
        selected={null}
        onMention={none}
        onDetails={none}
      />,
    );
    expect(html).toContain("240 more");
    expect(html).not.toContain("10@x.example");
  });

  it("says when there is nothing to review", () => {
    const client = new QueryClient();
    client.setQueryData(["review", null, "count"], 0);
    client.setQueryData(["review", null, "head"], { rows: [], next: null });
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <ReviewQueue
          entity={null}
          onShowAll={none}
          index={0}
          onIndex={none}
          mention={null}
          onMention={none}
          onOpenEntity={none}
          source={null}
        />
      </QueryClientProvider>,
    );
    expect(html).toContain("Nothing to review");
    expect(html).toContain("0 proposals");
  });

  it("names the entity it is narrowed to and offers the rest (US-07.7 AC4)", () => {
    const html = renderToStaticMarkup(
      <QueryClientProvider client={new QueryClient()}>
        <ReviewQueue
          entity={{ key: "person:corvin hale", name: "Corvin Hale" }}
          onShowAll={none}
          index={0}
          onIndex={none}
          mention={null}
          onMention={none}
          onOpenEntity={none}
          source={null}
        />
      </QueryClientProvider>,
    );
    expect(html).toContain("Only Corvin Hale");
    expect(html).toContain("Show all");
  });
});

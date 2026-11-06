import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ConnectionFilter } from "../ipc/bindings/ConnectionFilter";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import { DEFAULT_FILTER, KIND_CHOICES, kindChoice, kindsOf, linkedLabel } from "../lib/connections";
import { ConnectionSummary, FilterBar } from "./ConnectionsList";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;

const filter = (kinds: string[]): ConnectionFilter => ({ ...DEFAULT_FILTER, kinds });

const email = (value: string) => ({ key: `email:${value}`, kind: "email", value });

const person = (over: Partial<ConnectionItem> = {}): ConnectionItem => ({
  key: "person:corvin hale",
  kind: "person",
  value: "Corvin Hale",
  label: null,
  fromTable: false,
  sourceCount: 3,
  fileTypeCount: 2,
  mentionCount: 7,
  fileTypes: ["eml", "txt"],
  lowConfidence: false,
  lowOnly: false,
  ignored: false,
  linkedIdentifiers: [],
  linkedTotal: 0,
  ...over,
});

describe("the type filter (US-07.7 AC1)", () => {
  it("offers All, People, Organizations and Identifiers", () => {
    const html = renderToStaticMarkup(
      <QueryClientProvider client={new QueryClient()}>
        <FilterBar filter={DEFAULT_FILTER} fileTypes={[]} ignored={0} onChange={none} />
      </QueryClientProvider>,
    );
    for (const label of ["All types", "People", "Organizations", "Identifiers"]) {
      expect(html).toContain(`>${label}<`);
    }
    expect(html).not.toContain("Name (from table)");
  });

  it("Identifiers stands for every identifier type and for no name", () => {
    const kinds = kindsOf("identifiers");
    for (const kind of ["email", "phone", "phone_unnormalized", "iban", "custom"]) {
      expect(kinds).toContain(kind);
    }
    expect(kinds).not.toContain("person");
    expect(kinds).not.toContain("org");
  });

  it("shows the choice whose whole kind list matches, not the one that has its first kind", () => {
    for (const choice of KIND_CHOICES) {
      expect(kindChoice(filter(kindsOf(choice.id)))).toBe(choice.id);
    }
    expect(kindChoice(filter(["email"]))).toBe("email");
    expect(kindChoice(filter([]))).toBe("");
  });
});

describe("an entity row (US-07.7 AC2)", () => {
  it("shows name, type, sources, mentions and its linked identifiers", () => {
    const html = renderToStaticMarkup(
      <ConnectionSummary
        row={person({ linkedIdentifiers: [email("corvin@x.example")], linkedTotal: 1 })}
        selected={false}
      />,
    );
    for (const text of ["Corvin Hale", "Person", "3 sources", "7 mentions", "corvin@x.example"]) {
      expect(html).toContain(text);
    }
  });

  it("counts the identifiers it does not show", () => {
    const row = { linkedIdentifiers: [email("a@x.example"), email("b@x.example")], linkedTotal: 5 };
    expect(linkedLabel(row)).toBe("a@x.example, b@x.example +3 more");
    expect(linkedLabel({ linkedIdentifiers: [], linkedTotal: 0 })).toBeNull();
  });

  it("keeps the second line on one line when it has linked identifiers", () => {
    const linked = renderToStaticMarkup(
      <ConnectionSummary
        row={person({ linkedIdentifiers: [email("a@x.example")], linkedTotal: 1 })}
        selected={false}
      />,
    );
    expect(linked).toContain("whitespace-nowrap");
    expect(linked).toContain("truncate font-mono");
    const plain = renderToStaticMarkup(<ConnectionSummary row={person()} selected={false} />);
    expect(plain).not.toContain("whitespace-nowrap");
  });

  it("marks an entity from a table, and not one a model also found", () => {
    const table = renderToStaticMarkup(
      <ConnectionSummary row={person({ fromTable: true })} selected={false} />,
    );
    expect(table).toContain("Name (from table)");
    const model = renderToStaticMarkup(<ConnectionSummary row={person()} selected={false} />);
    expect(model).not.toContain("Name (from table)");
  });
});

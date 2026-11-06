import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { ConnectionsSummary } from "../ipc/bindings/ConnectionsSummary";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import { commands } from "../ipc/commands";
import {
  DEFAULT_FILTER,
  ariaSort,
  emptyMessage,
  groupByDocument,
  isFiltered,
  kindChoice,
  kindsOf,
  newConnections,
  nextSort,
} from "../lib/connections";
import { staleKeys } from "../lib/events";
import { ConnectionSummary, FilterBar, NewConnectionsBar, SortHeader } from "./ConnectionsList";
import { OccurrencesPanel } from "./OccurrencesPanel";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;

const row = (over: Partial<ConnectionItem> = {}): ConnectionItem => ({
  key: "email:ann@x.example",
  kind: "email",
  value: "ann@x.example",
  label: null,
  fromTable: false,
  sourceCount: 3,
  fileTypeCount: 3,
  mentionCount: 5,
  fileTypes: ["csv", "eml", "pdf"],
  lowConfidence: false,
  lowOnly: false,
  ignored: false,
  linkedIdentifiers: [],
  linkedTotal: 0,
  ...over,
});

const occurrence = (over: Partial<Occurrence>): Occurrence => ({
  mentionId: 1,
  key: "email:ann@x.example",
  fileType: "csv",
  path: "/src/people.csv",
  location: { kind: "cell", row: 3, columnName: "email", columnIndex: null },
  raw: "ann@x.example",
  lowConfidence: false,
  confidence: 1,
  aiModel: null,
  documentId: 10,
  snippet: [
    { text: "Ann, ", hit: false },
    { text: "ann@x.example", hit: true },
  ],
  ...over,
});

const summary = (over: Partial<ConnectionsSummary> = {}): ConnectionsSummary => ({
  visible: 0,
  ignored: 0,
  documentsDone: 12,
  documentsTotal: 12,
  ...over,
});

describe("a connection row (US-07.1 AC2)", () => {
  it("lists value, type, counts and file types", () => {
    const html = renderToStaticMarkup(<ConnectionSummary row={row()} selected={false} />);
    expect(html).toContain("ann@x.example");
    expect(html).toContain("3 sources");
    expect(html).toContain("5 mentions");
    for (const t of ["csv", "eml", "pdf"]) expect(html).toMatch(new RegExp(`uppercase[^>]*>${t}<`));
    expect(html).not.toContain("Ignored");
  });

  it("badges low confidence and a name from a table", () => {
    const html = renderToStaticMarkup(
      <ConnectionSummary row={row({ lowConfidence: true, fromTable: true })} selected={false} />,
    );
    expect(html).toContain("Low confidence");
    expect(html).toContain("Name (from table)");
  });

  it("offers Ignore, and Un-ignore on an ignored row, greyed (US-07.4)", () => {
    const live = renderToStaticMarkup(
      <ConnectionSummary row={row()} selected={false} onIgnore={none} />,
    );
    expect(live).toContain('aria-label="Ignore ann@x.example"');
    expect(live).not.toContain("opacity-60");
    const ignored = renderToStaticMarkup(
      <ConnectionSummary row={row({ ignored: true })} selected={false} onIgnore={none} />,
    );
    expect(ignored).toContain('aria-label="Un-ignore ann@x.example"');
    expect(ignored).toContain("opacity-60");
    expect(ignored).toContain("Ignored");
  });

  it("has one button on a row: Ignore", () => {
    const html = renderToStaticMarkup(
      <ConnectionSummary row={row()} selected={false} onIgnore={none} />,
    );
    expect(html.match(/<button/g)).toHaveLength(1);
  });
});

describe("filters, find and sort (US-07.1 AC3, US-07.2)", () => {
  it("labels each control and counts the ignored ones", () => {
    const html = renderToStaticMarkup(
      <FilterBar filter={DEFAULT_FILTER} fileTypes={["csv", "pdf"]} ignored={4} onChange={none} />,
    );
    for (const label of ["Type", "Sources", "File type", "Find", "Show ignored (4)"]) {
      expect(html).toContain(label);
    }
    expect(html).toContain("People");
    expect(html).toContain(">PDF<");
    expect(html).not.toContain("Clear filters");
  });

  it("offers Clear filters once something narrows the list", () => {
    const html = renderToStaticMarkup(
      <FilterBar
        filter={{ ...DEFAULT_FILTER, find: "ann" }}
        fileTypes={[]}
        ignored={0}
        onChange={none}
      />,
    );
    expect(html).toContain("Clear filters");
    expect(isFiltered({ ...DEFAULT_FILTER, includeIgnored: true })).toBe(false);
  });

  it("maps a type choice to its connection types, phone with its digits-only form", () => {
    expect(kindsOf("phone")).toEqual(["phone", "phone_unnormalized"]);
    expect(kindsOf("")).toEqual([]);
    expect(kindChoice({ ...DEFAULT_FILTER, kinds: ["phone_unnormalized", "phone"] })).toBe("phone");
    expect(kindChoice(DEFAULT_FILTER)).toBe("");
  });

  it("cycles a column: first click, flip, back to the default ranking", () => {
    let f = { sort: DEFAULT_FILTER.sort, descending: false };
    f = nextSort(f, "value");
    expect(f).toEqual({ sort: "value", descending: false });
    f = nextSort(f, "value");
    expect(f).toEqual({ sort: "value", descending: true });
    f = nextSort(f, "value");
    expect(f).toEqual({ sort: "rank", descending: false });
    // Numbers start with the biggest.
    f = nextSort(f, "sources");
    expect(f).toEqual({ sort: "sources", descending: true });
    f = nextSort(f, "sources");
    expect(f).toEqual({ sort: "sources", descending: false });
  });

  it("marks the sorted column with aria-sort", () => {
    const f = { ...DEFAULT_FILTER, sort: "sources" as const, descending: true };
    expect(ariaSort(f, "sources")).toBe("descending");
    expect(ariaSort(f, "value")).toBe("none");
    const html = renderToStaticMarkup(<SortHeader filter={f} onChange={none} />);
    expect(html).toMatch(/aria-sort="descending"[^>]*><button[^>]*>Sources ↓/);
    expect(html.match(/aria-sort="none"/g)).toHaveLength(4);
  });

  it("sends the filter with the page and the count", async () => {
    invoke.mockClear();
    const filter = { ...DEFAULT_FILTER, kinds: ["email"], find: "ann", includeIgnored: true };
    await commands.connectionsPage(filter, null, 200);
    expect(invoke).toHaveBeenCalledWith("connections_page", { filter, cursor: null, limit: 200 });
    await commands.connectionCount(filter);
    expect(invoke).toHaveBeenCalledWith("connection_count", { filter });
    await commands.setIgnored("email:a@x.example", true);
    expect(invoke).toHaveBeenCalledWith("set_ignored", {
      key: "email:a@x.example",
      ignored: true,
    });
  });
});

describe("new connections while reading (US-07.5)", () => {
  it("shows the bar only when there are new ones", () => {
    expect(renderToStaticMarkup(<NewConnectionsBar count={0} onShow={none} />)).toBe("");
    const html = renderToStaticMarkup(<NewConnectionsBar count={3} onShow={none} />);
    expect(html).toContain("3 new connections");
    expect(html).toContain(">Show<");
    expect(renderToStaticMarkup(<NewConnectionsBar count={1} onShow={none} />)).toContain(
      "1 new connection<",
    );
  });

  it("counts the live total against the shown one, never below zero", () => {
    expect(newConnections(15, 12)).toBe(3);
    expect(newConnections(10, 12)).toBe(0);
  });

  it("refreshes the summary on the events that change connections", () => {
    expect(staleKeys({ kind: "connectionsChanged" })).toContainEqual(["progress"]);
  });
});

describe("empty states (US-07.6)", () => {
  it("says nothing when there are rows", () => {
    expect(emptyMessage(summary(), 5, false)).toBeNull();
  });
  it("says how far the reading is while files are still going", () => {
    expect(emptyMessage(summary({ documentsDone: 3, documentsTotal: 10 }), 0, false)).toBe(
      "Looking for connections… 3 of 10 files done",
    );
  });
  it("says none were found, with the document count", () => {
    expect(emptyMessage(summary({ documentsDone: 12 }), 0, false)).toBe(
      "No cross-source connections found in 12 documents.",
    );
    expect(emptyMessage(summary({ documentsDone: 1, documentsTotal: 1 }), 0, false)).toBe(
      "No cross-source connections found in 1 document.",
    );
  });
  it("tells a filter that matches nothing from an empty project", () => {
    expect(emptyMessage(summary({ visible: 4 }), 0, true)).toBe(
      "No connections match these filters.",
    );
  });
  it("tells when everything is ignored", () => {
    expect(emptyMessage(summary({ ignored: 2 }), 0, false)).toContain("ignored");
  });
});

describe("evidence by document (US-07.3 AC1, NFR-USE-04)", () => {
  const items = [
    occurrence({ mentionId: 1 }),
    occurrence({ mentionId: 2, location: { kind: "line", line: 9 } }),
    occurrence({
      mentionId: 3,
      documentId: 11,
      fileType: "eml",
      path: "/src/mail.eml",
      location: { kind: "header", name: "To" },
    }),
  ];

  it("groups in the core's order, one group per document", () => {
    const groups = groupByDocument(items);
    expect(groups.map((g) => [g.documentId, g.occurrences.length])).toEqual([
      [10, 2],
      [11, 1],
    ]);
  });

  it("shows each document with its type, places and highlighted snippets; one button per mention", () => {
    const client = new QueryClient();
    client.setQueryData(["occurrences", "email:ann@x.example", false], items);
    const html = renderToStaticMarkup(
      <QueryClientProvider client={client}>
        <OccurrencesPanel connection={row()} selected={2} onSelect={none} />
      </QueryClientProvider>,
    );
    expect(html).toContain("people.csv");
    expect(html).toContain("mail.eml");
    expect(html).toContain("line 9");
    expect(html).toContain("header To");
    expect(html).toMatch(/<mark data-hit="true"[^>]*>ann@x\.example<\/mark>/);
    expect(html.match(/<button/g)).toHaveLength(3);
    expect(html.match(/<h3/g)).toHaveLength(2);
  });
});

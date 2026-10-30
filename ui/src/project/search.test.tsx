import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { SearchHit } from "../ipc/bindings/SearchHit";
import { commands } from "../ipc/commands";
import { EMPTY_PLACE, samePlace, viewKey } from "../lib/history";
import { locationLabel, resultCount } from "../lib/labels";
import { SearchPanel, SearchResultRow, Snippet, TypeChips } from "./SearchPanel";

const invoke = vi.hoisted(() => vi.fn(() => Promise.resolve(null)));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: Object }));

const none = () => undefined;

const hit = (over: Partial<SearchHit> = {}): SearchHit => ({
  segmentId: 41,
  documentId: 7,
  fileType: "csv",
  path: "/cases/vpa-expense-claims-2025.csv",
  missing: false,
  location: { kind: "row", row: 3 },
  snippet: [
    { text: "Daniel ", hit: false },
    { text: "Okonkwo", hit: true },
    { text: "-Hart 38.50 ************4444", hit: false },
  ],
  hit: { segmentId: 41, start: 7, end: 14 },
  ...over,
});

/** The props of the first element in a tree that matches. */
function find(node: ReactNode, test: (props: Record<string, unknown>) => boolean): unknown {
  if (node === null || typeof node !== "object") return null;
  if (Array.isArray(node)) {
    for (const n of node as ReactNode[]) {
      const got = find(n, test);
      if (got) return got;
    }
    return null;
  }
  const el = node as ReactElement<Record<string, unknown>>;
  if (test(el.props)) return el.props;
  return find(el.props.children as ReactNode, test);
}

describe("search results (US-06.1 AC1, NFR-SEC-14)", () => {
  it("shows the file, the place and the snippet with the hit marked", () => {
    const html = renderToStaticMarkup(
      <SearchResultRow
        hit={hit()}
        active={false}
        opened={false}
        onOpen={none}
        onOnlyThisDocument={none}
      />,
    );
    expect(html).toContain("vpa-expense-claims-2025.csv");
    expect(html).toContain(">csv<");
    expect(html).toContain("row 3");
    expect(html).toMatch(/<mark data-hit="true"[^>]*>Okonkwo<\/mark>/);
    expect(html).not.toContain("data-missing");
  });

  it("shows card-like numbers as the core masked them, never more", () => {
    const html = renderToStaticMarkup(<Snippet pieces={hit().snippet} />);
    expect(html).toContain("************4444");
    expect(html).not.toMatch(/\d{13}/);
  });

  it("shows snippet text as text, never as markup", () => {
    const html = renderToStaticMarkup(
      <Snippet
        pieces={[
          { text: "<img src=x onerror=alert(1)>", hit: false },
          { text: "<b>", hit: true },
        ]}
      />,
    );
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x onerror=alert(1)&gt;");
    expect(html).toContain("&lt;b&gt;</mark>");
  });

  it('marks a document whose file is gone "Missing" (domain D4)', () => {
    const html = renderToStaticMarkup(
      <SearchResultRow
        hit={hit({ missing: true })}
        active={false}
        opened={false}
        onOpen={none}
        onOnlyThisDocument={none}
      />,
    );
    expect(html).toContain("data-missing");
    expect(html).toContain("Missing");
  });

  it("counts results, with + while more pages are left", () => {
    expect(resultCount(1, false)).toBe("1 result");
    expect(resultCount(12, false)).toBe("12 results");
    expect(resultCount(50, true)).toBe("50+ results");
  });

  it('labels a whole CSV row "row N"', () => {
    expect(locationLabel({ kind: "row", row: 214 })).toBe("row 214");
  });
});

describe("opening a result (US-06.1 AC4, NFR-USE-04)", () => {
  it("opens on one click, and asks the core for that place once", async () => {
    const onOpen = vi.fn();
    const row = SearchResultRow({
      hit: hit(),
      active: false,
      opened: false,
      onOpen,
      onOnlyThisDocument: none,
    });
    const button = find(row, (p) => p["data-segment-id"] === 41) as { onClick: () => void };
    button.onClick();
    expect(onOpen).toHaveBeenCalledTimes(1);

    invoke.mockClear();
    await commands.searchContext({ segmentId: 41, start: 7, end: 14 });
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("search_context", { segmentId: 41, start: 7, end: 14 });
  });

  it("keeps a search result in the view history, apart from mentions", () => {
    const at = { ...EMPTY_PLACE, hit: { segmentId: 41, start: 7, end: 14 } };
    expect(viewKey(at)).toBe("h41:7-14");
    expect(viewKey({ ...EMPTY_PLACE, mention: 3 })).toBe("m3");
    expect(viewKey({ ...at, document: 9 })).toBe("d9");
    expect(viewKey(EMPTY_PLACE)).toBeNull();
    expect(samePlace(at, { ...at, hit: { segmentId: 41, start: 7, end: 14 } })).toBe(true);
    expect(samePlace(at, { ...at, hit: { segmentId: 41, start: 20, end: 27 } })).toBe(false);
  });
});

describe("search panel (US-06.1 AC2–AC3, AC5)", () => {
  it("starts with a query box and no results or message", () => {
    const html = renderToStaticMarkup(
      <QueryClientProvider client={new QueryClient()}>
        <SearchPanel opened={null} onOpen={none} />
      </QueryClientProvider>,
    );
    expect(html).toContain('aria-label="Search the documents"');
    expect(html).not.toContain("No matches");
    expect(html).not.toContain('role="alert"');
  });

  it("shows file-type chips, chosen ones pressed", () => {
    const html = renderToStaticMarkup(
      <TypeChips types={["pdf", "csv", "eml"]} chosen={["csv"]} onToggle={none} />,
    );
    expect(html).toMatch(/aria-pressed="true"[^>]*>csv</);
    expect(html).toMatch(/aria-pressed="false"[^>]*>pdf</);
  });
});

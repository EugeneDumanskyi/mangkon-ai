import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { SearchCursor } from "../ipc/bindings/SearchCursor";
import type { SearchHit } from "../ipc/bindings/SearchHit";
import type { SearchQuery } from "../ipc/bindings/SearchQuery";
import type { SegmentHit } from "../ipc/bindings/SegmentHit";
import type { TextPiece } from "../ipc/bindings/TextPiece";
import { needsNextPage } from "../lib/keyset";
import { fileName, locationLabel, resultCount } from "../lib/labels";

/** Results per page: the first 50 (NFR-PERF-30). */
export const SEARCH_PAGE = 50;
const ROW_HEIGHT = 84;
const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** A document the search is narrowed to (US-06.1 AC3), with what to call it. */
export interface DocumentScope {
  id: number;
  name: string;
}

/** The text of a search hit: plain text nodes, the matched words in `<mark>` (no HTML). */
export function Snippet({ pieces }: { pieces: TextPiece[] }) {
  return (
    <span className="line-clamp-2 text-xs">
      {pieces.map((p, i) =>
        p.hit ? (
          <mark key={i} data-hit className="rounded-sm bg-accent px-0.5 text-accent-text">
            {p.text}
          </mark>
        ) : (
          <span key={i}>{p.text}</span>
        ),
      )}
    </span>
  );
}

/** One search result (US-06.1 AC1): the file, where in it, the snippet; one click opens it. */
export function SearchResultRow({
  hit,
  active,
  opened,
  onOpen,
  onOnlyThisDocument,
}: {
  hit: SearchHit;
  /** The row the keyboard is on. */
  active: boolean;
  /** The row shown in the source pane. */
  opened: boolean;
  onOpen: () => void;
  onOnlyThisDocument: () => void;
}) {
  return (
    <div
      className={`flex h-full items-start gap-2 border-b border-border px-3 py-1.5 ${
        opened ? "bg-accent/15" : ""
      } ${active ? "outline-2 -outline-offset-2 outline-accent" : ""}`}
    >
      <button
        type="button"
        tabIndex={-1}
        className="min-w-0 flex-1 text-left text-sm"
        title={hit.path ?? undefined}
        data-segment-id={hit.segmentId}
        onClick={onOpen}
      >
        <span className="flex items-baseline gap-2">
          <span className="truncate font-semibold">{fileName(hit.path)}</span>
          <span className="shrink-0 text-xs uppercase text-muted">{hit.fileType}</span>
          {hit.missing && (
            <span className="shrink-0 rounded border border-border px-1 text-xs" data-missing>
              Missing
            </span>
          )}
        </span>
        <span className="block text-xs text-muted">{locationLabel(hit.location)}</span>
        <Snippet pieces={hit.snippet} />
      </button>
      <button
        type="button"
        tabIndex={-1}
        className="shrink-0 text-xs text-muted underline"
        onClick={onOnlyThisDocument}
      >
        Only this document
      </button>
    </div>
  );
}

/** File-type chips (US-06.1 AC3): none chosen searches every type. */
export function TypeChips({
  types,
  chosen,
  onToggle,
}: {
  types: string[];
  chosen: string[];
  onToggle: (fileType: string) => void;
}) {
  return (
    <span className="flex flex-wrap items-center gap-1" aria-label="File types">
      {types.map((t) => {
        const on = chosen.includes(t);
        return (
          <button
            key={t}
            type="button"
            aria-pressed={on}
            className={`rounded-full border px-2 py-0.5 text-xs uppercase ${
              on ? "border-accent bg-accent text-accent-text" : "border-border text-muted"
            }`}
            onClick={() => {
              onToggle(t);
            }}
          >
            {t}
          </button>
        );
      })}
    </span>
  );
}

/**
 * Keyword search over every document's stored text (US-06.1): Enter runs the query, file-type
 * chips and "Only this document" narrow it, ↑/↓ move through the results and Enter opens one
 * (NFR-USE-06). A result opens in the source pane in one click (AC4, NFR-USE-04).
 */
export function SearchPanel({
  opened,
  onOpen,
}: {
  /** The result shown in the source pane. */
  opened: SegmentHit | null;
  onOpen: (hit: SearchHit) => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [text, setText] = useState("");
  const [query, setQuery] = useState<SearchQuery | null>(null);
  const [fileTypes, setFileTypes] = useState<string[]>([]);
  const [scope, setScope] = useState<DocumentScope | null>(null);
  const [active, setActive] = useState(0);
  const types = useQuery({ queryKey: ["documents", "types"], queryFn: commands.documentTypes });

  const run = (next: { text?: string; fileTypes?: string[]; scope?: DocumentScope | null }) => {
    const t = next.text ?? query?.text ?? text;
    const ft = next.fileTypes ?? fileTypes;
    const sc = next.scope === undefined ? scope : next.scope;
    setFileTypes(ft);
    setScope(sc);
    setActive(0);
    // Enter always runs (an empty query gets its plain message); a filter change re-runs a
    // query already made.
    if (next.text !== undefined || query !== null) {
      setQuery({ text: t, fileTypes: ft, document: sc?.id ?? null });
    }
  };

  const pages = useInfiniteQuery({
    queryKey: ["search", query],
    initialPageParam: null as SearchCursor | null,
    queryFn: ({ pageParam }) =>
      query === null
        ? Promise.resolve({ hits: [], next: null })
        : commands.search(query, pageParam, SEARCH_PAGE),
    getNextPageParam: (last) => last.next,
    enabled: query !== null,
    retry: false,
  });
  const rows = pages.data?.pages.flatMap((p) => p.hits) ?? [];
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = pages;
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; nothing here is memoized.
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 8,
  });
  const items = virtualizer.getVirtualItems();
  const lastVisible = Math.max(items.at(-1)?.index ?? 0, active);
  useEffect(() => {
    if (needsNextPage(lastVisible, rows.length, hasNextPage, isFetchingNextPage)) {
      void fetchNextPage();
    }
  }, [lastVisible, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  const open = (i: number) => {
    const hit = rows[i];
    if (hit === undefined) return;
    setActive(i);
    onOpen(hit);
  };
  const move = (to: number) => {
    const i = Math.max(0, Math.min(rows.length - 1, to));
    setActive(i);
    virtualizer.scrollToIndex(i, { align: "auto" });
  };
  const isOpened = (h: SearchHit) =>
    opened !== null &&
    h.hit !== null &&
    h.hit.segmentId === opened.segmentId &&
    h.hit.start === opened.start &&
    h.hit.end === opened.end;

  const loaded = pages.data !== undefined;
  return (
    <section className="flex min-h-0 flex-col border-r border-border" aria-label="Search">
      <div className="flex flex-col gap-2 border-b border-border px-3 py-2 text-sm">
        <input
          type="search"
          aria-label="Search the documents"
          placeholder='Words, "exact phrase", prefix*, a OR b'
          className="rounded border border-border bg-surface px-2 py-1"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              run({ text });
            } else if (e.key === "ArrowDown" && rows.length > 0) {
              e.preventDefault();
              listRef.current?.focus();
              move(0);
            }
          }}
        />
        <TypeChips
          types={(types.data ?? []).map((t) => t.fileType)}
          chosen={fileTypes}
          onToggle={(t) => {
            run({
              fileTypes: fileTypes.includes(t)
                ? fileTypes.filter((x) => x !== t)
                : [...fileTypes, t],
            });
          }}
        />
        {scope !== null && (
          <span className="flex items-center gap-2">
            <span className="min-w-0 truncate">
              Only in <span className="font-semibold">{scope.name}</span>
            </span>
            <button
              type="button"
              className={button}
              aria-label="Search all documents"
              onClick={() => {
                run({ scope: null });
              }}
            >
              ✕
            </button>
          </span>
        )}
        {loaded && rows.length > 0 && (
          <span className="text-xs text-muted">{resultCount(rows.length, hasNextPage)}</span>
        )}
      </div>
      {pages.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(pages.error)}
        </p>
      )}
      {loaded && rows.length === 0 && !pages.error && (
        <p className="px-3 py-2 text-sm text-muted">No matches</p>
      )}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto" data-scroll="search">
        <div
          ref={listRef}
          role="listbox"
          aria-label="Search results"
          tabIndex={rows.length > 0 ? 0 : -1}
          aria-activedescendant={rows.length > 0 ? `search-row-${String(active)}` : undefined}
          className="relative focus:outline-none"
          style={{ height: virtualizer.getTotalSize() }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              move(active + (e.key === "ArrowDown" ? 1 : -1));
            } else if (e.key === "Enter") {
              e.preventDefault();
              open(active);
            }
          }}
        >
          {items.map((item) => {
            const hit = rows[item.index];
            if (hit === undefined) return null;
            return (
              <div
                key={item.key}
                id={`search-row-${String(item.index)}`}
                role="option"
                aria-selected={item.index === active}
                className="absolute left-0 w-full"
                style={{ top: item.start, height: ROW_HEIGHT }}
              >
                <SearchResultRow
                  hit={hit}
                  active={item.index === active}
                  opened={isOpened(hit)}
                  onOpen={() => {
                    open(item.index);
                  }}
                  onOnlyThisDocument={() => {
                    run({ scope: { id: hit.documentId, name: fileName(hit.path) } });
                  }}
                />
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

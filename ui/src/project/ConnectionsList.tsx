import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionCursor } from "../ipc/bindings/ConnectionCursor";
import type { ConnectionFilter } from "../ipc/bindings/ConnectionFilter";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import {
  DEFAULT_FILTER,
  KIND_CHOICES,
  SORT_COLUMNS,
  ariaSort,
  emptyMessage,
  isFiltered,
  kindChoice,
  kindsOf,
  linkedLabel,
  newConnectionsLabel,
  nextSort,
} from "../lib/connections";
import { onCoreEvent } from "../lib/coreEvents";
import { PAGE_SIZE } from "../lib/keyset";
import { kindLabel, kindWarning, plural } from "../lib/labels";
import { percentile } from "../lib/stats";
import { useKeysetList } from "../lib/useKeysetList";

const ROW_HEIGHT = 60;
const FIND_DELAY_MS = 250;
const focusRing =
  "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent";
const small = `rounded border border-border px-2 py-0.5 text-xs disabled:opacity-50 ${focusRing}`;

/**
 * One connection's value, type, counts and file types (US-07.1 AC2), with a note on a phone kept
 * as digits (US-04.2 AC3). `onIgnore` adds the Ignore / Un-ignore button (US-07.4).
 */
export function ConnectionSummary({
  row,
  selected,
  onIgnore,
  onReview,
}: {
  row: ConnectionItem;
  selected: boolean;
  onIgnore?: (row: ConnectionItem) => void;
  /** Opens the review queue for this entity (US-07.7 AC4). */
  onReview?: (row: ConnectionItem) => void;
}) {
  const warning = kindWarning(row.kind);
  const linked = linkedLabel(row);
  const muted = selected ? "text-xs" : "text-xs text-muted";
  return (
    <div className={row.ignored ? "opacity-60" : undefined} data-ignored={row.ignored || undefined}>
      <div className="flex items-baseline gap-2">
        <span
          className={
            row.kind === "person" || row.kind === "org" ? "truncate" : "truncate font-mono"
          }
        >
          {row.value}
        </span>
        {row.fromTable && (
          <span className="shrink-0 rounded border border-current px-1 text-xs">
            Name (from table)
          </span>
        )}
        {(row.lowConfidence || row.lowOnly) && (
          <span className="shrink-0 rounded border border-current px-1 text-xs">
            Low confidence
          </span>
        )}
        {row.ignored && (
          <span className="shrink-0 rounded border border-current px-1 text-xs">Ignored</span>
        )}
        {row.unreviewed &&
          (onReview ? (
            <button
              type="button"
              tabIndex={-1}
              className="shrink-0 rounded border border-current px-1 text-xs underline"
              onClick={(e) => {
                e.stopPropagation();
                onReview(row);
              }}
            >
              Unreviewed merge
            </button>
          ) : (
            <span className="shrink-0 rounded border border-current px-1 text-xs">
              Unreviewed merge
            </span>
          ))}
        {warning !== null && (
          <span
            className="shrink-0 rounded border border-current px-1 text-xs"
            title="Set the phone region in Settings to match it with the same number in other forms."
          >
            {warning}
          </span>
        )}
        {onIgnore && (
          <button
            type="button"
            tabIndex={-1}
            className={`${small} ml-auto shrink-0 ${selected ? "border-current" : ""}`}
            aria-label={`${row.ignored ? "Un-ignore" : "Ignore"} ${row.value}`}
            onClick={(e) => {
              e.stopPropagation();
              onIgnore(row);
            }}
          >
            {row.ignored ? "Un-ignore" : "Ignore"}
          </button>
        )}
      </div>
      <div
        className={`flex items-center gap-x-1 ${linked === null ? "flex-wrap" : "whitespace-nowrap"} ${muted}`}
      >
        <span className={linked === null ? undefined : "shrink-0"}>
          {row.label ?? kindLabel(row.kind)} · {plural(row.sourceCount, "source")} ·{" "}
          {plural(row.mentionCount, "mention")}
        </span>
        <span
          aria-label={`File types: ${row.fileTypes.join(", ")}`}
          className={linked === null ? "flex gap-1" : "flex shrink-0 gap-1"}
        >
          {row.fileTypes.map((t) => (
            <span key={t} className="rounded border border-current px-1 uppercase">
              {t}
            </span>
          ))}
        </span>
        {linked !== null && (
          <span className="min-w-0 truncate font-mono" title={linked}>
            · {linked}
          </span>
        )}
      </div>
    </div>
  );
}

/** The filter bar (US-07.1 AC3, US-07.2, US-07.4): type, minimum sources, file type, find. */
export function FilterBar({
  filter,
  fileTypes,
  ignored,
  findRef,
  onChange,
}: {
  filter: ConnectionFilter;
  fileTypes: string[];
  ignored: number;
  findRef?: React.Ref<HTMLInputElement>;
  onChange: (next: Partial<ConnectionFilter>) => void;
}) {
  const [text, setText] = useState(filter.find);
  // A cleared filter empties the box too.
  const [seen, setSeen] = useState(filter.find);
  if (seen !== filter.find) {
    setSeen(filter.find);
    setText(filter.find);
  }
  useEffect(() => {
    if (text === filter.find) return;
    const timer = setTimeout(() => {
      onChange({ find: text });
    }, FIND_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
    // `onChange` is a new function each render; the box's text is what starts the delay.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);
  const field = `rounded border border-border bg-transparent px-1 py-0.5 text-xs ${focusRing}`;
  return (
    <div
      role="search"
      aria-label="Filter connections"
      className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-2"
    >
      <label className="flex items-center gap-1 text-xs">
        Type
        <select
          className={field}
          value={kindChoice(filter)}
          onChange={(e) => {
            onChange({ kinds: kindsOf(e.target.value) });
          }}
        >
          <option value="">All types</option>
          {KIND_CHOICES.map((c) => (
            <option key={c.id} value={c.id}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-center gap-1 text-xs">
        Sources
        <select
          className={field}
          value={filter.minSources}
          onChange={(e) => {
            onChange({ minSources: Number(e.target.value) });
          }}
        >
          <option value={0}>Any</option>
          {[3, 4, 5].map((n) => (
            <option key={n} value={n}>
              {n}+
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-center gap-1 text-xs">
        File type
        <select
          className={field}
          value={filter.fileTypes[0] ?? ""}
          onChange={(e) => {
            onChange({ fileTypes: e.target.value === "" ? [] : [e.target.value] });
          }}
        >
          <option value="">All</option>
          {fileTypes.map((t) => (
            <option key={t} value={t}>
              {t.toUpperCase()}
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-center gap-1 text-xs">
        Find
        <input
          ref={findRef}
          type="search"
          className={`${field} w-40`}
          placeholder="Find a value ( / )"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
          }}
        />
      </label>
      <label className="flex items-center gap-1 text-xs">
        <input
          type="checkbox"
          checked={filter.includeIgnored}
          onChange={(e) => {
            onChange({ includeIgnored: e.target.checked });
          }}
        />
        Show ignored{ignored > 0 ? ` (${ignored.toLocaleString()})` : ""}
      </label>
      <label className="flex items-center gap-1 text-xs">
        <input
          type="checkbox"
          checked={filter.includeLow}
          onChange={(e) => {
            onChange({ includeLow: e.target.checked });
          }}
        />
        Show low confidence
      </label>
      {(isFiltered(filter) || filter.includeIgnored || filter.includeLow) && (
        <button
          type="button"
          className={small}
          onClick={() => {
            onChange({ ...DEFAULT_FILTER, sort: filter.sort, descending: filter.descending });
          }}
        >
          Clear filters
        </button>
      )}
    </div>
  );
}

/** Sortable column headers (US-07.2): a click sorts, again flips, a third restores the ranking. */
export function SortHeader({
  filter,
  onChange,
}: {
  filter: ConnectionFilter;
  onChange: (next: Partial<ConnectionFilter>) => void;
}) {
  return (
    <div role="table" aria-label="Sort connections" className="border-b border-border">
      <div role="row" className="flex gap-1 px-3 py-1 text-xs">
        <span className="text-muted">Sort by</span>
        {SORT_COLUMNS.map((c) => {
          const sort = ariaSort(filter, c.id);
          return (
            <div key={c.id} role="columnheader" aria-sort={sort}>
              <button
                type="button"
                className={`rounded px-1 ${sort === "none" ? "" : "font-semibold underline"} ${focusRing}`}
                onClick={() => {
                  onChange(nextSort(filter, c.id));
                }}
              >
                {c.label}
                {sort === "ascending" ? " ↑" : sort === "descending" ? " ↓" : ""}
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** "N new connections — Show" (US-07.5): the list holds still until the user asks. */
export function NewConnectionsBar({ count, onShow }: { count: number; onShow: () => void }) {
  if (count <= 0) return null;
  return (
    <div
      role="status"
      className="flex items-center gap-2 border-b border-border bg-accent px-3 py-1 text-sm text-accent-text"
    >
      <span>{newConnectionsLabel(count)}</span>
      <button
        type="button"
        className={`rounded border border-current px-2 text-xs ${focusRing}`}
        onClick={onShow}
      >
        Show
      </button>
    </div>
  );
}

/**
 * NFR-PERF-34 screen side (`MK_UI_TIMING=1`): from a `connectionsChanged` event to the frame
 * after the list re-rendered with the new rows. Saved when an ingest run finishes.
 */
function useUpdateTiming(enabled: boolean, updatedAt: number) {
  const pending = useRef<number | null>(null);
  const samples = useRef<number[]>([]);
  useEffect(() => {
    if (!enabled) return;
    return onCoreEvent((event, at) => {
      if (event.kind === "connectionsChanged") pending.current ??= at;
      if (event.kind === "ingestFinished" && samples.current.length > 0) {
        const sorted = [...samples.current].sort((a, b) => a - b);
        void commands.recordUiTiming({
          samples: samples.current,
          p50Ms: percentile(sorted, 50),
          p95Ms: percentile(sorted, 95),
          maxMs: sorted.at(-1) ?? 0,
          userAgent: navigator.userAgent,
        });
      }
    });
  }, [enabled]);
  useEffect(() => {
    const start = pending.current;
    if (!enabled || start === null) return;
    pending.current = null;
    requestAnimationFrame(() => {
      samples.current.push(performance.now() - start);
    });
  }, [enabled, updatedAt]);
}

/**
 * The connections screen (E07): filters, find, sort, ignore, and a list that holds still while
 * the user is reading it (US-07.5), virtualized over keyset pages (ADR-0003).
 */
export function ConnectionsList({
  selectedKey,
  onSelect,
  onReview,
  uiTiming,
}: {
  selectedKey: string | null;
  onSelect: (c: ConnectionItem) => void;
  onReview: (c: ConnectionItem) => void;
  uiTiming: boolean;
}) {
  const client = useQueryClient();
  const scrollRef = useRef<HTMLDivElement>(null);
  const findRef = useRef<HTMLInputElement>(null);
  const [filter, setFilter] = useState<ConnectionFilter>(DEFAULT_FILTER);
  const [active, setActive] = useState(0);
  const [lastVisible, setLastVisible] = useState(0);
  const [scrolled, setScrolled] = useState(false);
  // The list holds still while the user has scrolled or has a connection open (US-07.5).
  const paused = scrolled || selectedKey !== null;
  const list = useKeysetList<ConnectionItem, ConnectionCursor>({
    queryKey: ["connections", filter],
    count: () => commands.connectionCount(filter),
    page: (cursor) => commands.connectionsPage(filter, cursor, PAGE_SIZE),
    lastVisible,
    paused,
  });
  const summary = useQuery({
    queryKey: ["progress", "summary"],
    queryFn: commands.connectionsSummary,
  });
  const types = useQuery({ queryKey: ["documents", "types"], queryFn: commands.documentTypes });
  useUpdateTiming(uiTiming, list.updatedAt);
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; nothing here is memoized.
  const virtualizer = useVirtualizer({
    count: list.total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();
  const last = items.at(-1)?.index ?? 0;
  useEffect(() => {
    setLastVisible(last);
  }, [last]);

  const toTop = () => {
    scrollRef.current?.scrollTo({ top: 0 });
    setScrolled(false);
    setActive(0);
  };
  const change = (next: Partial<ConnectionFilter>) => {
    setFilter((f) => ({ ...f, ...next }));
    toTop();
  };
  const move = (to: number) => {
    const next = Math.max(0, Math.min(list.total - 1, to));
    setActive(next);
    virtualizer.scrollToIndex(next);
  };
  const choose = (index: number) => {
    const row = list.rowAt(index);
    if (row) onSelect(row);
  };
  const ignore = (row: ConnectionItem) => {
    void commands.setIgnored(row.key, !row.ignored).then(async () => {
      await list.refresh();
      await client.invalidateQueries({ queryKey: ["progress", "summary"] });
    });
  };
  const newCount = Math.max(0, list.liveTotal - list.total);
  const message = emptyMessage(summary.data, list.total, isFiltered(filter));

  return (
    <section
      className="flex min-h-0 flex-col overflow-y-auto border-r border-border"
      aria-label="Connections"
      onKeyDown={(e) => {
        const target = e.target as HTMLElement;
        const typing = ["INPUT", "SELECT", "TEXTAREA"].includes(target.tagName);
        if (e.key === "/" && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) {
          e.preventDefault();
          findRef.current?.focus();
        } else if (e.key === "Escape" && target === findRef.current) {
          scrollRef.current?.focus();
        }
      }}
    >
      <h2 className="border-b border-border px-3 py-2 text-sm font-semibold">
        {plural(list.total, "connection")}
      </h2>
      <FilterBar
        filter={filter}
        fileTypes={(types.data ?? []).map((t) => t.fileType)}
        ignored={summary.data?.ignored ?? 0}
        findRef={findRef}
        onChange={change}
      />
      <SortHeader filter={filter} onChange={change} />
      <NewConnectionsBar
        count={newCount}
        onShow={() => {
          toTop();
          void list.refresh();
        }}
      />
      {list.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(list.error)}
        </p>
      )}
      {message !== null && !list.error && (
        <p className="px-3 py-2 text-sm text-muted" role="status">
          {message}
        </p>
      )}
      <div
        ref={scrollRef}
        data-scroll="connections"
        className="min-h-24 flex-1 overflow-auto focus:outline-2 focus:outline-accent"
        role="listbox"
        aria-label="Connections list"
        aria-activedescendant={list.total > 0 ? `connection-${String(active)}` : undefined}
        tabIndex={0}
        onScroll={(e) => {
          setScrolled(e.currentTarget.scrollTop > 0);
        }}
        onKeyDown={(e) => {
          const page = Math.max(1, Math.floor((scrollRef.current?.clientHeight ?? 0) / ROW_HEIGHT));
          const keys: Record<string, number> = {
            ArrowDown: active + 1,
            ArrowUp: active - 1,
            PageDown: active + page,
            PageUp: active - page,
            Home: 0,
            End: list.total - 1,
          };
          const to = keys[e.key];
          if (to !== undefined) {
            e.preventDefault();
            move(to);
          } else if (e.key === "Enter") {
            e.preventDefault();
            choose(active);
          } else if (e.key === "i" && !e.ctrlKey && !e.metaKey && !e.altKey) {
            const row = list.rowAt(active);
            if (row) {
              e.preventDefault();
              ignore(row);
            }
          }
        }}
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {items.map((item) => {
            const row = list.rowAt(item.index);
            const isSelected = row !== undefined && row.key === selectedKey;
            return (
              <div
                key={item.key}
                id={`connection-${String(item.index)}`}
                role="option"
                aria-selected={isSelected}
                aria-posinset={item.index + 1}
                aria-setsize={list.total}
                className={`absolute left-0 top-0 w-full cursor-pointer border-b border-border px-3 py-1.5 text-sm ${
                  isSelected ? "bg-accent text-accent-text" : ""
                } ${item.index === active ? "outline-2 -outline-offset-2 outline-accent" : ""}`}
                style={{ height: ROW_HEIGHT, transform: `translateY(${String(item.start)}px)` }}
                onClick={() => {
                  setActive(item.index);
                  choose(item.index);
                }}
              >
                {row ? (
                  <ConnectionSummary
                    row={row}
                    selected={isSelected}
                    onIgnore={ignore}
                    onReview={onReview}
                  />
                ) : (
                  <span className="text-muted">…</span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

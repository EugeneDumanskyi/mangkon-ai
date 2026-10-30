import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionCursor } from "../ipc/bindings/ConnectionCursor";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import { onCoreEvent } from "../lib/coreEvents";
import { PAGE_SIZE } from "../lib/keyset";
import { kindLabel, kindWarning, plural } from "../lib/labels";
import { percentile } from "../lib/stats";
import { useKeysetList } from "../lib/useKeysetList";

const ROW_HEIGHT = 56;

/** One connection's value and counts, with a note on a phone kept as digits (US-04.2 AC3). */
export function ConnectionSummary({ row, selected }: { row: ConnectionItem; selected: boolean }) {
  const warning = kindWarning(row.kind);
  return (
    <>
      <div className="flex items-baseline gap-2">
        <span className="truncate font-mono">{row.value}</span>
        {row.lowConfidence && (
          <span className="shrink-0 rounded border border-current px-1 text-xs">
            low confidence
          </span>
        )}
        {warning !== null && (
          <span
            className="shrink-0 rounded border border-current px-1 text-xs"
            title="Set the phone region in Settings to match it with the same number in other forms."
          >
            {warning}
          </span>
        )}
      </div>
      <div className={selected ? "text-xs" : "text-xs text-muted"}>
        {kindLabel(row.kind)} · {plural(row.sourceCount, "source")} ·{" "}
        {plural(row.mentionCount, "mention")} · {plural(row.fileTypeCount, "file type")}
      </div>
    </>
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

/** The raw connections list (US-07.1 AC1/AC2), virtualized over keyset pages (ADR-0003). */
export function ConnectionsList({
  selectedKey,
  onSelect,
  uiTiming,
}: {
  selectedKey: string | null;
  onSelect: (c: ConnectionItem) => void;
  uiTiming: boolean;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState(0);
  const [lastVisible, setLastVisible] = useState(0);
  const list = useKeysetList<ConnectionItem, ConnectionCursor>({
    queryKey: ["connections"],
    count: commands.connectionCount,
    page: (cursor) => commands.connectionsPage(cursor, PAGE_SIZE),
    lastVisible,
  });
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

  const move = (to: number) => {
    const next = Math.max(0, Math.min(list.total - 1, to));
    setActive(next);
    virtualizer.scrollToIndex(next);
  };
  const choose = (index: number) => {
    const row = list.rowAt(index);
    if (row) onSelect(row);
  };

  return (
    <section className="flex min-h-0 flex-col border-r border-border" aria-label="Connections">
      <h2 className="border-b border-border px-3 py-2 text-sm font-semibold">
        {plural(list.total, "connection")}
      </h2>
      {list.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(list.error)}
        </p>
      )}
      {list.total === 0 && !list.error && (
        <p className="px-3 py-2 text-sm text-muted">
          No connections yet. A connection is an identifier found in two or more files.
        </p>
      )}
      <div
        ref={scrollRef}
        data-scroll="connections"
        className="min-h-0 flex-1 overflow-auto focus:outline-2 focus:outline-accent"
        role="listbox"
        aria-label="Connections"
        aria-activedescendant={list.total > 0 ? `connection-${String(active)}` : undefined}
        tabIndex={0}
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
                  <ConnectionSummary row={row} selected={isSelected} />
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

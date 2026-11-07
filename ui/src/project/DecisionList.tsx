import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { DecisionItem } from "../ipc/bindings/DecisionItem";
import { needsNextPage, PAGE_SIZE } from "../lib/keyset";
import {
  decisionChange,
  decisionKindLabel,
  decisionStatusLabel,
  decisionTime,
} from "../lib/labels";

const ROW_HEIGHT = 56;

/** One decision: when, what, before → after, whether it still applies (US-11.5 AC2). */
export function DecisionRow({
  decision,
  onOpenEntity,
}: {
  decision: DecisionItem;
  onOpenEntity: (key: string) => void;
}) {
  const change = decisionChange(decision);
  const status = decisionStatusLabel(decision);
  const entity = decision.entityKey;
  return (
    <div data-decision={decision.kind} className="flex flex-col px-3 py-1 text-sm">
      <div className="flex items-baseline gap-2">
        <span className="shrink-0 font-semibold">{decisionKindLabel(decision)}</span>
        <span className="min-w-0 flex-1 truncate" title={change}>
          {change}
        </span>
        {entity !== null && (
          <button
            type="button"
            className="shrink-0 rounded border border-border px-2 text-xs"
            onClick={() => {
              onOpenEntity(entity);
            }}
          >
            Open
          </button>
        )}
      </div>
      <div className="truncate text-xs text-muted" title={status}>
        {decisionTime(decision.decidedAt)}
        {status === "" ? "" : ` · ${status}`}
      </div>
    </div>
  );
}

/** The decision log in project settings, newest first (US-11.5 AC2), in keyset pages (ADR-0003). */
export function DecisionList({ onOpenEntity }: { onOpenEntity: (key: string) => void }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const pages = useInfiniteQuery({
    queryKey: ["decisions"],
    initialPageParam: null as number | null,
    queryFn: ({ pageParam }) => commands.decisionsPage(pageParam, PAGE_SIZE),
    getNextPageParam: (last) => last.next,
  });
  const rows = pages.data?.pages.flatMap((p) => p.rows) ?? [];
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = pages;
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; nothing here is memoized.
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();
  const last = items.at(-1)?.index ?? 0;
  useEffect(() => {
    if (needsNextPage(last, rows.length, hasNextPage, isFetchingNextPage)) void fetchNextPage();
  }, [last, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  return (
    <section aria-label="Decisions" className="max-w-3xl">
      <h3 className="mt-6 font-semibold">Your decisions</h3>
      {pages.error && (
        <p role="alert" className="text-sm">
          {errorMessage(pages.error)}
        </p>
      )}
      {pages.isSuccess && rows.length === 0 && (
        <p className="text-sm text-muted">No decisions yet.</p>
      )}
      {rows.length > 0 && (
        <div
          ref={scrollRef}
          className="mt-1 h-80 overflow-auto rounded border border-border"
          data-scroll="decisions"
          tabIndex={0}
          aria-label="Decisions, newest first"
        >
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {items.map((item) => {
              const row = rows[item.index];
              return (
                <div
                  key={item.key}
                  className="absolute left-0 top-0 w-full border-b border-border"
                  style={{ height: ROW_HEIGHT, transform: `translateY(${String(item.start)}px)` }}
                >
                  {row && <DecisionRow decision={row} onOpenEntity={onOpenEntity} />}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </section>
  );
}

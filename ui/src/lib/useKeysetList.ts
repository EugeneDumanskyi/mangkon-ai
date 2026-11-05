import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { heldLength, needsNextPage } from "./keyset";

export interface KeysetPage<Row, Cursor> {
  rows: Row[];
  next: Cursor | null;
}

/**
 * A long list read in keyset pages (ADR-0003). Its length comes from `count`, so the scrollbar is
 * right before the rows arrive; rows not loaded yet are `undefined` (placeholders). Pages load
 * in order as the view reaches them.
 *
 * While `paused`, a refresh of the data (a core event) does not touch the rows or their count
 * (US-07.5): `total` stays what it was and `liveTotal` follows the data, so a caller can offer
 * "N new". `refresh()` loads the current data and ends the hold.
 */
export function useKeysetList<Row, Cursor>(options: {
  queryKey: readonly unknown[];
  count: () => Promise<number>;
  page: (cursor: Cursor | null) => Promise<KeysetPage<Row, Cursor>>;
  lastVisible: number;
  paused?: boolean;
}) {
  const { queryKey, count, page, lastVisible, paused = false } = options;
  const client = useQueryClient();
  const pagesKey = [...queryKey, "pages"];
  const total = useQuery({ queryKey: [...queryKey, "count"], queryFn: count });
  // A list never read yet must load even when paused (a filter changed under an open row).
  const loaded = client.getQueryData(pagesKey) !== undefined;
  const pages = useInfiniteQuery({
    queryKey: pagesKey,
    initialPageParam: null as Cursor | null,
    queryFn: ({ pageParam }) => page(pageParam as Cursor | null),
    getNextPageParam: (last) => last.next,
    enabled: !paused || !loaded,
  });
  const rows = pages.data?.pages.flatMap((p) => p.rows) ?? [];
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = pages;

  // The length the rows were loaded for, taken when the hold starts or the list changes.
  const live = total.data ?? 0;
  const listKey = JSON.stringify(queryKey);
  const [held, setHeld] = useState(heldLength(null, paused, total.data));
  const [seen, setSeen] = useState({ listKey, paused });
  if (seen.listKey !== listKey || seen.paused !== paused) {
    setSeen({ listKey, paused });
    setHeld(heldLength(null, paused, total.data));
  } else if (heldLength(held, paused, total.data) !== held) {
    setHeld(heldLength(held, paused, total.data));
  }

  useEffect(() => {
    if (needsNextPage(lastVisible, rows.length, hasNextPage, isFetchingNextPage)) {
      void fetchNextPage();
    }
  }, [lastVisible, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  const shown = paused && held !== null ? held : live;
  return {
    total: shown,
    liveTotal: live,
    rowAt: (index: number): Row | undefined => rows[index],
    error: total.error ?? pages.error,
    /** Bumps when a refetch lands, so a caller can time its re-render. */
    updatedAt: Math.max(total.dataUpdatedAt, pages.dataUpdatedAt),
    /** Loads the current rows and count now, also while paused. */
    refresh: async () => {
      const fresh = await total.refetch();
      await pages.refetch();
      setHeld(fresh.data ?? null);
    },
  };
}

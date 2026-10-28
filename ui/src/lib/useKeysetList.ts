import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { needsNextPage } from "./keyset";

export interface KeysetPage<Row, Cursor> {
  rows: Row[];
  next: Cursor | null;
}

/**
 * A long list read in keyset pages (ADR-0003). Its length comes from `count`, so the scrollbar is
 * right before the rows arrive; rows not loaded yet are `undefined` (placeholders). Pages load
 * in order as the view reaches them.
 */
export function useKeysetList<Row, Cursor>(options: {
  queryKey: readonly unknown[];
  count: () => Promise<number>;
  page: (cursor: Cursor | null) => Promise<KeysetPage<Row, Cursor>>;
  lastVisible: number;
}) {
  const { queryKey, count, page, lastVisible } = options;
  const total = useQuery({ queryKey: [...queryKey, "count"], queryFn: count });
  const pages = useInfiniteQuery({
    queryKey: [...queryKey, "pages"],
    initialPageParam: null as Cursor | null,
    queryFn: ({ pageParam }) => page(pageParam as Cursor | null),
    getNextPageParam: (last) => last.next,
  });
  const rows = pages.data?.pages.flatMap((p) => p.rows) ?? [];
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = pages;

  useEffect(() => {
    if (needsNextPage(lastVisible, rows.length, hasNextPage, isFetchingNextPage)) {
      void fetchNextPage();
    }
  }, [lastVisible, rows.length, hasNextPage, isFetchingNextPage, fetchNextPage]);

  return {
    total: total.data ?? 0,
    rowAt: (index: number): Row | undefined => rows[index],
    error: total.error ?? pages.error,
    /** Bumps when a refetch lands, so a caller can time its re-render. */
    updatedAt: Math.max(total.dataUpdatedAt, pages.dataUpdatedAt),
  };
}

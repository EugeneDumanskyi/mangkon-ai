import { useQueries, useQuery } from "@tanstack/react-query";
import { commands } from "../ipc/commands";
import type { RowPage } from "../ipc/bindings/RowPage";
import type { SyntheticRow } from "../ipc/bindings/SyntheticRow";
import { PAGE_SIZE, afterKeyForPage, pagesForRange } from "./paging";

function pageQuery(page: number, onFetch: (ms: number) => void) {
  return {
    queryKey: ["synthetic_rows", page],
    queryFn: async (): Promise<RowPage> => {
      const t0 = performance.now();
      const result = await commands.syntheticRows({
        afterKey: afterKeyForPage(page),
        limit: PAGE_SIZE,
      });
      onFetch(performance.now() - t0);
      return result;
    },
  };
}

/** The list length, from the first page (shared with the row cache). */
export function useRowTotal(onFetch: (ms: number) => void): number {
  return useQuery(pageQuery(0, onFetch)).data?.total ?? 0;
}

/** Loads the pages behind rows `first..=last` and gives each loaded row by position. */
export function useKeysetRows(first: number, last: number, onFetch: (ms: number) => void) {
  const pages = pagesForRange(first, last);
  const results = useQueries({ queries: pages.map((p) => pageQuery(p, onFetch)) });
  const loaded = new Map<number, RowPage>();
  pages.forEach((p, i) => {
    const data = results[i]?.data;
    if (data) loaded.set(p, data);
  });

  return (index: number): SyntheticRow | undefined =>
    loaded.get(Math.floor(index / PAGE_SIZE))?.rows[index % PAGE_SIZE];
}

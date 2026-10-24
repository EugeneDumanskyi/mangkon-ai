// Keyset paging over the core's row lists (ADR-0003): the UI asks for pages of rows after a key
// and never holds the whole list.
//
// Spike S2d shortcut: synthetic row keys equal row positions, so the page for any scroll position
// can be fetched directly ("seek"). Real lists need a position → key lookup for long jumps (M2d).

export const PAGE_SIZE = 200;

/** Pages that hold rows `first..=last`. */
export function pagesForRange(first: number, last: number, pageSize = PAGE_SIZE): number[] {
  if (last < first) return [];
  const pages: number[] = [];
  for (let p = Math.floor(first / pageSize); p <= Math.floor(last / pageSize); p++) pages.push(p);
  return pages;
}

/** The `afterKey` that starts page `page`: the key of the last row of the page before. */
export function afterKeyForPage(page: number, pageSize = PAGE_SIZE): number | null {
  return page === 0 ? null : page * pageSize - 1;
}

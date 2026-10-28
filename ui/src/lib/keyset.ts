// Keyset paging over the core's row lists (ADR-0003): the UI asks for pages of rows after a cursor
// and never holds more of a list than it has scrolled through.

export const PAGE_SIZE = 200;

/** Pages that hold rows `first..=last`. */
export function pagesForRange(first: number, last: number, pageSize = PAGE_SIZE): number[] {
  if (last < first) return [];
  const pages: number[] = [];
  for (let p = Math.floor(first / pageSize); p <= Math.floor(last / pageSize); p++) pages.push(p);
  return pages;
}

/** Rows to keep loaded past the last visible one before asking for the next page. */
export const PREFETCH_ROWS = 50;

/**
 * Whether the next page is needed: the view reaches past the loaded rows (less `PREFETCH_ROWS`),
 * there is a next page, and none is on its way. Cursor paging cannot seek, so a long jump loads
 * the pages before it one after another.
 */
export function needsNextPage(
  lastVisible: number,
  loaded: number,
  hasNext: boolean,
  fetching: boolean,
): boolean {
  return hasNext && !fetching && lastVisible >= loaded - PREFETCH_ROWS;
}

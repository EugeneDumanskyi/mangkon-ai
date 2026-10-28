// Spike S2d shortcut over the shared keyset paging (src/lib/keyset.ts): synthetic row keys equal
// row positions, so the page for any scroll position can be fetched directly ("seek"). The real
// connections list pages by cursor (src/lib/useKeysetList.ts).
import { PAGE_SIZE } from "../lib/keyset";

export { PAGE_SIZE, pagesForRange } from "../lib/keyset";

/** The `afterKey` that starts page `page`: the key of the last row of the page before. */
export function afterKeyForPage(page: number, pageSize = PAGE_SIZE): number | null {
  return page === 0 ? null : page * pageSize - 1;
}

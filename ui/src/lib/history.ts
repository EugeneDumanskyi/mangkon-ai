// The view history of the project screen (US-08.2 AC2): each move to another connection,
// mention, document or search result pushes where the screen was, with its scroll offsets;
// "Back" pops it.
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { SegmentHit } from "../ipc/bindings/SegmentHit";

/** What the connections screen shows. */
export interface Place {
  connection: ConnectionItem | null;
  mention: number | null;
  /** A document opened from a link (an attachment or its email), over the mention. */
  document: number | null;
  /** A search result opened (US-06.1 AC4), instead of a mention. */
  hit: SegmentHit | null;
}

/** Scroll offsets of the three panes, in pixels. */
export interface Scrolls {
  connections: number;
  occurrences: number;
  source: { top: number; left: number };
}

export interface History {
  place: Place;
  back: { place: Place; scroll: Scrolls }[];
  /** Offsets to put back once the panes show `place` again; set by `back`. */
  restore: Scrolls | null;
}

/** Most places kept to go back to. */
export const HISTORY_LIMIT = 100;

export const EMPTY_PLACE: Place = { connection: null, mention: null, document: null, hit: null };

function sameHit(a: SegmentHit | null, b: SegmentHit | null): boolean {
  if (a === null || b === null) return a === b;
  return a.segmentId === b.segmentId && a.start === b.start && a.end === b.end;
}

/** What the source pane shows for `place`, as its `data-view` names it; `null` for nothing. */
export function viewKey(place: Place): string | null {
  if (place.document !== null) return `d${String(place.document)}`;
  if (place.hit !== null) {
    const h = place.hit;
    return `h${String(h.segmentId)}:${String(h.start)}-${String(h.end)}`;
  }
  if (place.mention !== null) return `m${String(place.mention)}`;
  return null;
}

export function samePlace(a: Place, b: Place): boolean {
  return (
    (a.connection?.key ?? null) === (b.connection?.key ?? null) &&
    a.mention === b.mention &&
    a.document === b.document &&
    sameHit(a.hit, b.hit)
  );
}

/** Goes to `place`, keeping where the screen was. Going where it already is does nothing. */
export function go(h: History, place: Place, scroll: Scrolls): History {
  if (samePlace(h.place, place)) return h;
  return {
    place,
    back: [...h.back, { place: h.place, scroll }].slice(-HISTORY_LIMIT),
    restore: null,
  };
}

/** Goes back to the last place, with its scroll offsets to restore. */
export function back(h: History): History {
  const last = h.back.at(-1);
  if (!last) return h;
  return { place: last.place, back: h.back.slice(0, -1), restore: last.scroll };
}

/** The offsets have been put back. */
export function restored(h: History): History {
  return h.restore === null ? h : { ...h, restore: null };
}

export const START: History = { place: EMPTY_PLACE, back: [], restore: null };

/** The panes' scroll containers carry `data-scroll="<pane>"`. */
export function readScrolls(root: ParentNode = document): Scrolls {
  const el = (pane: string) => root.querySelector<HTMLElement>(`[data-scroll="${pane}"]`);
  const source = el("source");
  return {
    connections: el("connections")?.scrollTop ?? 0,
    occurrences: el("occurrences")?.scrollTop ?? 0,
    source: { top: source?.scrollTop ?? 0, left: source?.scrollLeft ?? 0 },
  };
}

export function applyScrolls(s: Scrolls, root: ParentNode = document): void {
  const el = (pane: string) => root.querySelector<HTMLElement>(`[data-scroll="${pane}"]`);
  el("connections")?.scrollTo({ top: s.connections });
  el("occurrences")?.scrollTo({ top: s.occurrences });
  el("source")?.scrollTo({ top: s.source.top, left: s.source.left });
}

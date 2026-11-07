// Pure parts of the review queue (US-11.3): its keys, where it stands after a decision, whether
// the queue on screen is behind, and what "Edit" sends. No React here, so each is tested alone.
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { ProposalItem } from "../ipc/bindings/ProposalItem";

export type ReviewAction = "accept" | "reject" | "edit" | "next" | "previous";

/** `A` accept, `R` reject, `E` edit, `J`/`K` next/previous (US-11.3 AC5). */
const REVIEW_KEYS: Record<string, ReviewAction> = {
  a: "accept",
  r: "reject",
  e: "edit",
  j: "next",
  k: "previous",
};

/** The part of a key event the queue looks at. */
export interface ReviewKey {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  repeat: boolean;
  /** The tag name of the focused element, in capitals. */
  targetTag: string;
  targetEditable: boolean;
}

/**
 * What a key does in the queue, `null` for nothing: typing in a field and keys with a modifier are
 * not the queue's. Holding a key does not decide over and over, but it may move through the list.
 */
export function reviewAction(e: ReviewKey): ReviewAction | null {
  if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return null;
  if (e.targetEditable || ["INPUT", "SELECT", "TEXTAREA"].includes(e.targetTag)) return null;
  const action = REVIEW_KEYS[e.key.toLowerCase()] ?? null;
  if (e.repeat && action !== "next" && action !== "previous") return null;
  return action;
}

/** The row to show out of `total`: the same place, or the last when the list got shorter. */
export function clampIndex(index: number, total: number): number {
  return Math.max(0, Math.min(index, total - 1));
}

function samePair(a: ProposalItem | undefined, b: ProposalItem | undefined): boolean {
  return a?.a === b?.a && a?.b === b?.b;
}

/** Whether the data has moved on from the list on screen ("Queue updated — Show"). */
export function queueUpdated(
  live: { total: number; head: ProposalItem | undefined },
  shown: { total: number; head: ProposalItem | undefined },
): boolean {
  return live.total !== shown.total || !samePair(live.head, shown.head);
}

/** A proposal's score for people: 0.8612 is "0.86" (US-11.2 AC1). */
export function scoreText(score: number): string {
  return score.toFixed(2);
}

/** The names "Edit" offers: both sides' names and aliases, each once (US-11.3 AC4). */
export function knownNames(left: EntityDetail | null, right: EntityDetail | null): string[] {
  const names = new Set<string>();
  for (const side of [left, right]) {
    if (side === null) continue;
    names.add(side.name);
    for (const alias of side.aliases) names.add(alias.name);
  }
  return [...names];
}

/** What accepting would pick on its own: the more frequent side's name and type (US-11.3 AC2). */
export function defaultChoice(
  left: EntityDetail | null,
  right: EntityDetail | null,
): { name: string; entityType: string } {
  const first = (right?.mentionCount ?? 0) > (left?.mentionCount ?? 0) ? right : left;
  return { name: first?.name ?? "", entityType: first?.entityType ?? "person" };
}

/**
 * What "Accept" in the edit form sends: `null` for what was left as it would be anyway, so the log
 * holds only what the user changed.
 */
export function editedChoice(
  chosen: { name: string; entityType: string },
  defaults: { name: string; entityType: string },
): { name: string | null; entityType: string | null } {
  const name = chosen.name.trim();
  return {
    name: name === defaults.name ? null : name,
    entityType: chosen.entityType === defaults.entityType ? null : chosen.entityType,
  };
}

/** Evidence snippets per side in the queue (US-11.3 AC1). */
export const EVIDENCE_PER_SIDE = 5;
/** Evidence on an entity's page (`MAX_OCCURRENCES` in the core). */
export const EVIDENCE_ON_PAGE = 1_000;

/** "first 1,000 of 4,200" when a list stops before the end. */
export function firstOf(shown: number, total: number, what: string): string | null {
  return total > shown
    ? `Showing the first ${shown.toLocaleString()} of ${total.toLocaleString()} ${what}`
    : null;
}

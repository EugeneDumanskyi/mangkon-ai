// Pure parts of the Connections screen (E07): the filter, the sort headers, the empty-state
// messages and the grouping of evidence by document. No React here, so each is tested alone.
import type { ConnectionFilter } from "../ipc/bindings/ConnectionFilter";
import type { ConnectionSort } from "../ipc/bindings/ConnectionSort";
import type { ConnectionsSummary } from "../ipc/bindings/ConnectionsSummary";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import { plural } from "./labels";

/** No filter, the default ranking (domain §9.2), ignored connections hidden. */
export const DEFAULT_FILTER: ConnectionFilter = {
  kinds: [],
  minSources: 0,
  fileTypes: [],
  find: "",
  sort: "rank",
  descending: false,
  includeIgnored: false,
};

/** The type choices of the filter: a label and the connection types it stands for. */
export const KIND_CHOICES: { id: string; label: string; kinds: string[] }[] = [
  { id: "email", label: "Email", kinds: ["email"] },
  { id: "phone", label: "Phone", kinds: ["phone", "phone_unnormalized"] },
  { id: "url", label: "URL", kinds: ["url"] },
  { id: "domain", label: "Domain", kinds: ["domain"] },
  { id: "iban", label: "IBAN", kinds: ["iban"] },
  { id: "btc", label: "Bitcoin address", kinds: ["btc"] },
  { id: "evm", label: "Ethereum-style address", kinds: ["evm"] },
  { id: "sol", label: "Solana address", kinds: ["sol"] },
  { id: "tron", label: "TRON address", kinds: ["tron"] },
  { id: "custom", label: "Custom pattern", kinds: ["custom"] },
  { id: "person", label: "Name (from table): person", kinds: ["person"] },
  { id: "org", label: "Name (from table): organization", kinds: ["org"] },
];

/** The choice id a filter's types stand for, `""` for all. */
export function kindChoice(filter: ConnectionFilter): string {
  const first = filter.kinds[0];
  return KIND_CHOICES.find((c) => first !== undefined && c.kinds.includes(first))?.id ?? "";
}

/** The types of choice `id`; none for `""` (all). */
export function kindsOf(id: string): string[] {
  return KIND_CHOICES.find((c) => c.id === id)?.kinds ?? [];
}

/** Whether any filter, find text or "show ignored" narrows or widens the list. */
export function isFiltered(f: ConnectionFilter): boolean {
  return f.kinds.length > 0 || f.minSources > 0 || f.fileTypes.length > 0 || f.find.trim() !== "";
}

/** The sortable columns, in header order. */
export const SORT_COLUMNS: { id: ConnectionSort; label: string }[] = [
  { id: "value", label: "Value" },
  { id: "kind", label: "Type" },
  { id: "sources", label: "Sources" },
  { id: "mentions", label: "Mentions" },
  { id: "fileTypes", label: "File types" },
];

/**
 * What a click on a column header does (US-07.2): a new column sorts ascending (numbers: most
 * first), the same column again flips, a third click returns to the default ranking.
 */
export function nextSort(
  f: Pick<ConnectionFilter, "sort" | "descending">,
  column: ConnectionSort,
): Pick<ConnectionFilter, "sort" | "descending"> {
  const numeric = column === "sources" || column === "mentions" || column === "fileTypes";
  if (f.sort !== column) return { sort: column, descending: numeric };
  if (f.descending === numeric) return { sort: column, descending: !numeric };
  return { sort: "rank", descending: false };
}

export function ariaSort(
  f: Pick<ConnectionFilter, "sort" | "descending">,
  column: ConnectionSort,
): "ascending" | "descending" | "none" {
  if (f.sort !== column) return "none";
  return f.descending ? "descending" : "ascending";
}

/**
 * The message instead of an empty list (US-07.6), or `null` when the list has rows. `filtered`
 * is any narrowing in the filter bar.
 */
export function emptyMessage(
  summary: ConnectionsSummary | undefined,
  total: number,
  filtered: boolean,
): string | null {
  if (total > 0) return null;
  if (summary === undefined) return "Looking for connections…";
  if (summary.documentsDone < summary.documentsTotal && summary.visible === 0) {
    return `Looking for connections… ${summary.documentsDone.toLocaleString()} of ${summary.documentsTotal.toLocaleString()} files done`;
  }
  if (filtered && (summary.visible > 0 || summary.ignored > 0)) {
    return "No connections match these filters.";
  }
  if (summary.visible === 0 && summary.ignored > 0) {
    return "Every connection is ignored. Turn on Show ignored to see them.";
  }
  return `No cross-source connections found in ${plural(summary.documentsDone, "document")}.`;
}

/** How many connections came in while the list was held still (US-07.5). */
export function newConnections(live: number, shown: number): number {
  return Math.max(0, live - shown);
}

/** "3 new connections" for the bar. */
export function newConnectionsLabel(n: number): string {
  return plural(n, "new connection");
}

/** The evidence of a connection, one group per document (US-07.3 AC1), in the core's order. */
export interface EvidenceGroup {
  documentId: number;
  path: string | null;
  fileType: string;
  occurrences: Occurrence[];
}

export function groupByDocument(occurrences: Occurrence[]): EvidenceGroup[] {
  const groups: EvidenceGroup[] = [];
  const index = new Map<number, EvidenceGroup>();
  for (const o of occurrences) {
    let group = index.get(o.documentId);
    if (group === undefined) {
      group = { documentId: o.documentId, path: o.path, fileType: o.fileType, occurrences: [] };
      index.set(o.documentId, group);
      groups.push(group);
    }
    group.occurrences.push(o);
  }
  return groups;
}

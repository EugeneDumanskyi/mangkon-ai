import type { DecisionItem } from "../ipc/bindings/DecisionItem";
import type { Location } from "../ipc/bindings/Location";
import type { OriginalFile } from "../ipc/bindings/OriginalFile";

/**
 * "line 12", "row 3, column email", "row 3, column 2", "row 3", "paragraph 4", "$.customers[12].phone",
 * "header From", "body part 1.2", "page 3", "page 3 (OCR)" (1-based for people).
 */
export function locationLabel(location: Location): string {
  switch (location.kind) {
    case "line":
      return `line ${String(location.line)}`;
    case "cell": {
      const column =
        location.columnName ??
        (location.columnIndex === null ? "?" : String(location.columnIndex + 1));
      return `row ${String(location.row)}, column ${column}`;
    }
    case "row":
      return `row ${String(location.row)}`;
    case "paragraph":
      return `paragraph ${String(location.paragraph)}`;
    case "htmlText":
      return `text offset ${String(location.offset)}`;
    case "jsonPath":
      return location.path;
    case "header":
      return `header ${location.name}`;
    case "part":
      return `body part ${location.part}`;
    case "page":
      return `page ${String(location.page)}${location.ocr ? " (OCR)" : ""}`;
    case "other":
      return "place not shown in this version";
  }
}

const KIND_LABELS: Record<string, string> = {
  email: "email",
  phone: "phone",
  phone_unnormalized: "phone",
  url: "URL",
  domain: "domain",
  iban: "IBAN",
  btc: "Bitcoin address",
  evm: "Ethereum-style address",
  sol: "Solana address",
  tron: "TRON address",
  person: "Person",
  org: "Organization",
};

/** "IBAN", "Bitcoin address", … for an identifier type (US-04.1 – US-04.5). */
export function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

/**
 * The note on a phone number kept as written digits because the project has no phone region
 * (US-04.2 AC3): it only connects with the same digits, not with the number in another form.
 */
export function kindWarning(kind: string): string | null {
  return kind === "phone_unnormalized" ? "No region — exact digits only" : null;
}

/** The last part of a path, for either separator. */
export function fileName(path: string | null): string {
  if (path === null) return "(file no longer in the project)";
  const parts = path.split(/[\\/]/);
  return parts.at(-1) || path;
}

/** "12 results", or "50+ results" while there are more pages to load. */
export function resultCount(loaded: number, more: boolean): string {
  if (loaded === 1 && !more) return "1 result";
  return `${loaded.toLocaleString()}${more ? "+" : ""} results`;
}

/** "1,234 sources" with the right plural. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

/** Below this mean word confidence an OCR page is marked low quality (US-02.2 AC3). */
const LOW_OCR_QUALITY = 0.6;

/** "OCR quality 87%", "OCR quality 41% (low)". */
export function ocrQualityLabel(quality: number): string {
  const label = `OCR quality ${String(Math.round(quality * 100))}%`;
  return quality < LOW_OCR_QUALITY ? `${label} (low)` : label;
}

/** The text of the banner for an original that is not as it was read (US-08.5 AC1). */
export function originalNote(original: OriginalFile | null): string | null {
  if (original === null) return "Original file not found";
  switch (original.state) {
    case "present":
      return null;
    case "missing":
      return `Original file not found at ${original.path}`;
    case "changed":
      return "The file changed since it was read: showing the stored text";
  }
}

const MAPPING_STATES: Record<string, string> = {
  suggested: "Suggested",
  confirmed: "Confirmed",
  needs_review: "Needs review",
};

/** "Columns: Suggested / Confirmed / Needs review" of a CSV or JSON file (US-05.1, US-05.2). */
export function mappingStateLabel(state: string): string {
  return MAPPING_STATES[state] ?? state;
}

const COLUMN_TYPES: Record<string, string> = {
  email: "Email",
  phone: "Phone",
  url_domain: "URL / domain",
  iban: "IBAN",
  crypto: "Crypto address",
  person: "Person name",
  org: "Organization name",
  date: "Date",
  ignore: "Ignore",
};

/** The column types a user can pick, in order (INV-CM-01). */
export const COLUMN_TYPE_CHOICES = Object.keys(COLUMN_TYPES);

export function columnTypeLabel(type: string): string {
  return COLUMN_TYPES[type] ?? type;
}

/** What an entity's type is called: "person" is "Person", "org" is "Organization". */
export const ENTITY_TYPES = ["person", "org"];

/** The value part of a key: `person:corvin hale` is `corvin hale`. */
function keyValue(key: string): string {
  const at = key.indexOf(":");
  return at < 0 ? key : key.slice(at + 1);
}

const ALIAS_MARK = "#alias:";

const DECISION_KINDS: Record<string, string> = {
  ignore_identifier: "Ignored",
  unignore_identifier: "Un-ignored",
  ignore_entity: "Ignored",
  unignore_entity: "Un-ignored",
  confirm_mapping: "Confirmed columns",
  accept_merge: "Merged",
  reject_merge: "Rejected merge",
  split: "Split",
  undo_split: "Put back",
  rename: "Renamed",
  change_type: "Changed type",
  set_note: "Note",
  remove_alias: "Removed alias",
  restore_alias: "Restored alias",
};

/** A decision's kind in plain words (US-11.5 AC2). An undone merge is stored as a rejection. */
export function decisionKindLabel(d: Pick<DecisionItem, "kind" | "before">): string {
  if (d.kind === "reject_merge" && d.before === "merged") return "Undid merge";
  return DECISION_KINDS[d.kind] ?? d.kind;
}

function arrow(before: string | null, after: string | null): string {
  return before === null ? (after ?? "") : `${before} → ${after ?? ""}`;
}

/**
 * What a decision changed, before → after (US-11.5 AC2), in words a person can read: never the raw
 * keys a split lists or the JSON of a column mapping.
 */
export function decisionChange(d: DecisionItem): string {
  switch (d.kind) {
    case "split":
      return `${d.before ?? ""} → ${plural(d.moved ?? 0, "mention")} moved`;
    case "undo_split":
      return "Its mentions went back";
    case "confirm_mapping":
      return "Columns of a table file";
    case "change_type":
      return `${kindLabel(d.before ?? "")} → ${kindLabel(d.after ?? "")}`;
    case "set_note":
      return d.after === "" || d.after === null ? "Note cleared" : arrow(d.before, d.after);
    case "reject_merge": {
      const [a = "", b = ""] = d.anchor.split("|");
      return `${keyValue(a)} / ${keyValue(b)}: ${arrow(d.before, d.after)}`;
    }
    case "remove_alias":
    case "restore_alias": {
      const at = d.anchor.indexOf(ALIAS_MARK);
      const alias = at < 0 ? "" : d.anchor.slice(at + ALIAS_MARK.length);
      return `${alias}: ${arrow(d.before, d.after)}`;
    }
    case "ignore_identifier":
    case "unignore_identifier":
    case "ignore_entity":
    case "unignore_entity":
      return `${keyValue(d.anchor)}: ${arrow(d.before, d.after)}`;
    default:
      return arrow(d.before, d.after);
  }
}

/** Whether a decision still applies (domain §7.5). A mapping's state is its file's, so no text. */
export function decisionStatusLabel(d: Pick<DecisionItem, "kind" | "status">): string {
  switch (d.status) {
    case "active":
      return "Applies";
    case "superseded":
      return "Replaced by a newer decision";
    case "orphaned":
      // A merge with one name out of mentions holds: it applies again when they come back.
      return d.kind === "accept_merge" || d.kind === "reject_merge"
        ? "One of these names has no mentions now. It applies again when they come back."
        : "No longer applies — the source is gone or changed";
    default:
      return "";
  }
}

/** A decision's time, in the machine's own time zone. */
export function decisionTime(decidedAt: string): string {
  const time = new Date(decidedAt);
  return Number.isNaN(time.getTime()) ? decidedAt : time.toLocaleString();
}

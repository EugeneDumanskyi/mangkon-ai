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

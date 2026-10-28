import type { Location } from "../ipc/bindings/Location";

/** "line 12", "row 3, column email", "row 3, column 2" (1-based for people). */
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

/** "1,234 sources" with the right plural. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

import { describe, expect, it } from "vitest";
import { staleKeys } from "./events";
import { PREFETCH_ROWS, needsNextPage } from "./keyset";
import { fileName, locationLabel, plural } from "./labels";

describe("cursor paging", () => {
  it("asks for the next page only near the end of the loaded rows", () => {
    expect(needsNextPage(0, 200, true, false)).toBe(false);
    expect(needsNextPage(200 - PREFETCH_ROWS, 200, true, false)).toBe(true);
    expect(needsNextPage(5_000, 200, true, false)).toBe(true);
  });

  it("never asks past the last page or twice at once", () => {
    expect(needsNextPage(199, 200, false, false)).toBe(false);
    expect(needsNextPage(199, 200, true, true)).toBe(false);
  });

  it("asks for the first page of an empty list only if the core has one", () => {
    expect(needsNextPage(0, 0, true, false)).toBe(true);
    expect(needsNextPage(0, 0, false, false)).toBe(false);
  });
});

describe("locator labels", () => {
  it("labels lines and cells for people", () => {
    expect(locationLabel({ kind: "line", line: 12 })).toBe("line 12");
    expect(locationLabel({ kind: "cell", row: 3, columnName: "email", columnIndex: null })).toBe(
      "row 3, column email",
    );
    expect(locationLabel({ kind: "cell", row: 3, columnName: null, columnIndex: 0 })).toBe(
      "row 3, column 1",
    );
  });

  it("shows file names for both separators", () => {
    expect(fileName("/data/a/notes.txt")).toBe("notes.txt");
    expect(fileName("C:\\data\\b.csv")).toBe("b.csv");
    expect(fileName(null)).toContain("no longer");
    expect(plural(1, "source")).toBe("1 source");
    expect(plural(2, "source")).toBe("2 sources");
  });
});

describe("event → invalidation", () => {
  it("re-queries the list on connection changes, never with rows in the event", () => {
    expect(staleKeys({ kind: "connectionsChanged" })).toContainEqual(["connections"]);
    expect(staleKeys({ kind: "projectChanged" })).toContainEqual(["project"]);
    expect(staleKeys({ kind: "ingestStarted" })).toEqual([["progress"]]);
    expect(staleKeys({ kind: "ingestFinished", done: 1, failed: 0, error: null })).toContainEqual([
      "progress",
    ]);
    expect(staleKeys({ kind: "error", message: "x" })).toEqual([]);
  });
});

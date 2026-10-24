import { describe, expect, it } from "vitest";
import { afterKeyForPage, pagesForRange } from "./paging";
import { percentile, summarizeFrames } from "./stats";

describe("keyset paging", () => {
  it("covers a visible range with whole pages", () => {
    expect(pagesForRange(0, 0, 200)).toEqual([0]);
    expect(pagesForRange(190, 210, 200)).toEqual([0, 1]);
    expect(pagesForRange(99_950, 99_999, 200)).toEqual([499]);
    expect(pagesForRange(5, 4, 200)).toEqual([]);
  });

  it("starts each page after the last key of the page before", () => {
    expect(afterKeyForPage(0, 200)).toBeNull();
    expect(afterKeyForPage(1, 200)).toBe(199);
    expect(afterKeyForPage(499, 200)).toBe(99_799);
  });
});

describe("frame stats", () => {
  it("uses nearest-rank percentiles and strict limits", () => {
    expect(percentile([], 95)).toBe(0);
    expect(percentile([1, 2, 3, 4], 50)).toBe(2);
    const s = summarizeFrames([16.7, 16.6, 30, 10]);
    expect(s.frames).toBe(4);
    expect(s.maxMs).toBe(30);
    expect(s.meanMs).toBeCloseTo(18.325);
    expect(s.slowFrameShare).toBe(0.25);
    expect(s.jankFrameShare).toBe(0.25);
  });
});

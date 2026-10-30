import { describe, expect, it } from "vitest";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import { HISTORY_LIMIT, type Scrolls, START, back, go, restored } from "./history";

const conn = (key: string) => ({ key, value: key }) as unknown as ConnectionItem;
const at = (n: number): Scrolls => ({
  connections: n,
  occurrences: n + 1,
  source: { top: n + 2, left: n + 3 },
});

// US-08.2 AC2: Back returns to the screen I came from, at the same scroll position.
describe("view history (US-08.2 AC2)", () => {
  it("goes back through mentions and documents with their scroll offsets", () => {
    let h = go(START, { connection: conn("a"), mention: null, document: null, hit: null }, at(0));
    h = go(h, { connection: conn("a"), mention: 7, document: null, hit: null }, at(100));
    h = go(h, { connection: conn("a"), mention: 7, document: 3, hit: null }, at(200));
    h = go(h, { connection: conn("a"), mention: 9, document: null, hit: null }, at(300));

    h = back(h);
    expect(h.place).toEqual({ connection: conn("a"), mention: 7, document: 3, hit: null });
    expect(h.restore).toEqual(at(300));
    h = restored(h);
    expect(h.restore).toBeNull();

    h = back(h);
    expect(h.place).toEqual({ connection: conn("a"), mention: 7, document: null, hit: null });
    expect(h.restore).toEqual(at(200));

    h = back(back(h));
    expect(h.place).toEqual(START.place);
    expect(h.restore).toEqual(at(0));
    expect(back(h)).toBe(h);
  });

  it("does not stack the place it is already at", () => {
    const place = { connection: conn("a"), mention: 7, document: null, hit: null };
    const h = go(START, place, at(0));
    expect(go(h, { ...place, connection: conn("a") }, at(5))).toBe(h);
  });

  it(`keeps the last ${String(HISTORY_LIMIT)} places`, () => {
    let h = START;
    for (let i = 0; i < HISTORY_LIMIT + 20; i++) {
      h = go(h, { connection: null, mention: i, document: null, hit: null }, at(i));
    }
    expect(h.back).toHaveLength(HISTORY_LIMIT);
    expect(h.back.at(-1)?.scroll).toEqual(at(HISTORY_LIMIT + 19));
  });
});

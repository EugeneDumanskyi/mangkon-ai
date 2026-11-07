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
  it("goes back from an entity's page to where it was opened (US-12.1)", () => {
    const list = { ...START.place, connection: conn("a") };
    let h = go(START, list, at(0));
    h = go(h, { ...list, entity: "person:corvin hale" }, at(10));
    expect(h.place.entity).toBe("person:corvin hale");
    h = go(h, { ...h.place, mention: -4 }, at(20));
    h = back(h);
    expect(h.place).toEqual({ ...list, entity: "person:corvin hale" });
    expect(h.restore).toEqual(at(20));
    h = back(restored(h));
    expect(h.place).toEqual(list);
  });

  it("goes back through mentions and documents with their scroll offsets", () => {
    let h = go(
      START,
      { connection: conn("a"), mention: null, document: null, hit: null, entity: null },
      at(0),
    );
    h = go(
      h,
      { connection: conn("a"), mention: 7, document: null, hit: null, entity: null },
      at(100),
    );
    h = go(h, { connection: conn("a"), mention: 7, document: 3, hit: null, entity: null }, at(200));
    h = go(
      h,
      { connection: conn("a"), mention: 9, document: null, hit: null, entity: null },
      at(300),
    );

    h = back(h);
    expect(h.place).toEqual({
      connection: conn("a"),
      mention: 7,
      document: 3,
      hit: null,
      entity: null,
    });
    expect(h.restore).toEqual(at(300));
    h = restored(h);
    expect(h.restore).toBeNull();

    h = back(h);
    expect(h.place).toEqual({
      connection: conn("a"),
      mention: 7,
      document: null,
      hit: null,
      entity: null,
    });
    expect(h.restore).toEqual(at(200));

    h = back(back(h));
    expect(h.place).toEqual(START.place);
    expect(h.restore).toEqual(at(0));
    expect(back(h)).toBe(h);
  });

  it("does not stack the place it is already at", () => {
    const place = { connection: conn("a"), mention: 7, document: null, hit: null, entity: null };
    const h = go(START, place, at(0));
    expect(go(h, { ...place, connection: conn("a") }, at(5))).toBe(h);
  });

  it(`keeps the last ${String(HISTORY_LIMIT)} places`, () => {
    let h = START;
    for (let i = 0; i < HISTORY_LIMIT + 20; i++) {
      h = go(h, { connection: null, mention: i, document: null, hit: null, entity: null }, at(i));
    }
    expect(h.back).toHaveLength(HISTORY_LIMIT);
    expect(h.back.at(-1)?.scroll).toEqual(at(HISTORY_LIMIT + 19));
  });
});

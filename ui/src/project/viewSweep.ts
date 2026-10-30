// The scripted view sweeps, once ingestion ends; not used otherwise.
// - `MK_VIEW_SWEEP=1` (NFR-SEC-09): open the source view of every mention and every linked
//   document once, through the real panes, so a network trace of the app covers every view.
// - `MK_VIEW_SWEEP=clicks` (NFR-PERF-14, NFR-QA-12, US-08.2 AC2): click seeded random occurrence
//   rows and the fixed ones, time each click to the frame after the highlight is in the DOM,
//   check what it highlights, and go Next then Back to check the scroll comes back.
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionCursor } from "../ipc/bindings/ConnectionCursor";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { ClickSample } from "../ipc/bindings/ClickSample";
import type { ClickSweepInfo } from "../ipc/bindings/ClickSweepInfo";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import type { ViewSweep } from "../ipc/bindings/ViewSweep";
import { readScrolls } from "../lib/history";

const PANE = 'section[aria-label="Source"]';

function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      resolve();
    });
  });
}

/**
 * A page view's outcome: `true` once its image has loaded and the text toggle has shown the
 * text, `false` when the page fell back to text (status other than `shown`) or did not load in
 * 10 s.
 */
async function pageShown(el: HTMLElement): Promise<boolean> {
  const end = performance.now() + 10_000;
  while (performance.now() < end) {
    const status = el.querySelector<HTMLElement>("[data-page-status]")?.dataset.pageStatus;
    if (status !== undefined && status !== "shown") return false;
    const img = el.querySelector<HTMLImageElement>("img");
    if (img?.complete && img.naturalWidth > 0) {
      el.querySelector<HTMLElement>("[data-toggle-text]")?.click();
      while (performance.now() < end) {
        if (el.querySelector("[data-page-text]")) return true;
        await nextFrame();
      }
      return false;
    }
    await nextFrame();
  }
  return false;
}

/** The pane once it shows view `id` (`m<mention>` or `d<document>`), or `null` after 10 s. */
async function pane(id: string): Promise<HTMLElement | null> {
  const end = performance.now() + 10_000;
  while (performance.now() < end) {
    const el = document.querySelector<HTMLElement>(PANE);
    if (el?.dataset.view === id) return el;
    if (el?.dataset.view === `${id}-error`) return null;
    await nextFrame();
  }
  return null;
}

export async function runViewSweep(
  select: (connection: ConnectionItem, mentionId: number) => void,
): Promise<ViewSweep> {
  const sweep: ViewSweep = {
    mentions: 0,
    documents: 0,
    pages: 0,
    kinds: {},
    errors: [],
    clicks: [],
  };
  const count = async (el: HTMLElement, id: string) => {
    const kind = el.dataset.kind ?? "none";
    sweep.kinds[kind] = (sweep.kinds[kind] ?? 0) + 1;
    if (kind !== "page") return;
    if (await pageShown(el)) sweep.pages++;
    else sweep.errors.push(`page of ${id} not shown`);
  };
  const documents = new Set<string>();
  let cursor: ConnectionCursor | null = null;
  try {
    do {
      const page = await commands.connectionsPage(cursor, 500);
      for (const connection of page.rows) {
        for (const o of await commands.occurrences(connection.key)) {
          select(connection, o.mentionId);
          const shown = await pane(`m${String(o.mentionId)}`);
          if (!shown) {
            sweep.errors.push(`mention ${String(o.mentionId)} did not open`);
            continue;
          }
          sweep.mentions++;
          await count(shown, `mention ${String(o.mentionId)}`);
          // Every linked document (the parent email, the attachments), once.
          const links = [...shown.querySelectorAll<HTMLElement>("[data-document-id]")]
            .map((b) => b.dataset.documentId ?? "")
            .filter((id) => !documents.has(id));
          for (const id of links) {
            documents.add(id);
            document.querySelector<HTMLElement>(`${PANE} [data-document-id="${id}"]`)?.click();
            const doc = await pane(`d${id}`);
            if (!doc) {
              sweep.errors.push(`document ${id} did not open`);
              continue;
            }
            sweep.documents++;
            await count(doc, `document ${id}`);
            doc.querySelector<HTMLElement>("[data-back]")?.click();
            await pane(`m${String(o.mentionId)}`);
          }
        }
      }
      cursor = page.next;
    } while (cursor !== null);
  } catch (e) {
    sweep.errors.push(errorMessage(e));
  }
  return sweep;
}

/** A seeded generator (mulberry32): the same clicks on every run with the same seed. */
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Text compared without white space: a hit across lines is split per line. */
export function sameText(a: string, b: string): boolean {
  return a.replace(/\s+/g, "") === b.replace(/\s+/g, "");
}

/** Waits until `ready` gives a value, a frame at a time; `null` after `ms`. */
async function until<T>(ready: () => T | null | undefined, ms = 10_000): Promise<T | null> {
  const end = performance.now() + ms;
  while (performance.now() < end) {
    const v = ready();
    if (v !== null && v !== undefined) return v;
    await nextFrame();
  }
  return null;
}

/** The pane once it shows mention `id` with its highlight: text marks, or a loaded page image. */
function highlightShown(id: number): HTMLElement | null {
  const el = document.querySelector<HTMLElement>(PANE);
  if (el?.dataset.view !== `m${String(id)}`) return null;
  if (el.dataset.kind === "page") {
    const status = el.querySelector<HTMLElement>("[data-page-status]")?.dataset.pageStatus;
    if (status === undefined) return null;
    if (status !== "shown") return el.querySelector("[data-hit]") ? el : null;
    const img = el.querySelector<HTMLImageElement>("img");
    return img?.complete && img.naturalWidth > 0 && el.querySelector("[data-hit]") ? el : null;
  }
  return el.querySelector("[data-hit]") ? el : null;
}

/** The text the pane highlights; for a shown page, its text side's (toggled to read it). */
async function highlightedText(el: HTMLElement): Promise<string> {
  const read = () => [...el.querySelectorAll("mark[data-hit]")].map((m) => m.textContent).join("");
  if (el.dataset.kind !== "page" || el.querySelector("[data-page-text]")) return read();
  el.querySelector<HTMLElement>("[data-toggle-text]")?.click();
  await until(() => el.querySelector("[data-page-text]"));
  return read();
}

/** Next (or Previous), then Back: `true` when the mention and the scroll offsets came back. */
async function backComesBack(el: HTMLElement, id: number): Promise<boolean | null> {
  const step =
    el.querySelector<HTMLButtonElement>("[data-next]:not(:disabled)") ??
    el.querySelector<HTMLButtonElement>("[data-prev]:not(:disabled)");
  if (!step) return null;
  const before = readScrolls();
  step.click();
  const moved = await until(() => {
    const v = document.querySelector<HTMLElement>(PANE)?.dataset.view;
    return v !== undefined && v !== `m${String(id)}` && !v.endsWith("-error") ? v : null;
  });
  if (moved === null) return false;
  document.querySelector<HTMLElement>(`${PANE} [data-back]`)?.click();
  if ((await until(() => highlightShown(id))) === null) return false;
  // The screen puts the offsets back a few frames after the view is there.
  const near = (a: number, b: number) => Math.abs(a - b) <= 2;
  const back = await until(() => {
    const now = readScrolls();
    return near(now.source.top, before.source.top) &&
      near(now.source.left, before.source.left) &&
      near(now.occurrences, before.occurrences) &&
      near(now.connections, before.connections)
      ? true
      : null;
  }, 2_000);
  return back === true;
}

export async function runClickSweep(
  info: ClickSweepInfo,
  select: (connection: ConnectionItem) => void,
): Promise<ViewSweep> {
  const sweep: ViewSweep = {
    mentions: 0,
    documents: 0,
    pages: 0,
    kinds: {},
    errors: [],
    clicks: [],
  };
  try {
    const connections: ConnectionItem[] = [];
    let cursor: ConnectionCursor | null = null;
    do {
      const page = await commands.connectionsPage(cursor, 1_000);
      connections.push(...page.rows);
      cursor = page.next;
    } while (cursor !== null);
    if (connections.length === 0) throw new Error("The project has no connections.");

    // A random connection, then a random mention of it (each draw from the seed).
    const random = seeded(info.seed);
    const targets: { c: ConnectionItem; o: Occurrence; fixed: boolean }[] = [];
    for (let i = 0; i < info.n; i++) {
      const c = connections[Math.floor(random() * connections.length)];
      if (!c) continue;
      const rows = await commands.occurrences(c.key);
      const o = rows[Math.floor(random() * rows.length)];
      if (o) targets.push({ c, o, fixed: false });
    }
    for (const id of info.fixed) {
      const view = await commands.sourceContext(id);
      const key = view.occurrence?.key;
      const c = connections.find((x) => x.key === key);
      const o = c && (await commands.occurrences(c.key)).find((r) => r.mentionId === id);
      if (c && o) targets.push({ c, o, fixed: true });
      else sweep.errors.push(`fixed mention ${String(id)} is not in a connection`);
    }

    for (const { c, o, fixed } of targets) {
      const id = o.mentionId;
      select(c);
      const row = await until(() =>
        document.querySelector<HTMLElement>(
          `section[aria-label="Occurrences"] [data-mention-id="${String(id)}"]`,
        ),
      );
      if (!row) {
        sweep.errors.push(`mention ${String(id)}: row not listed`);
        continue;
      }
      row.scrollIntoView({ block: "nearest" });
      await nextFrame();
      const start = performance.now();
      row.click();
      const el = await until(() => highlightShown(id));
      if (!el) {
        sweep.errors.push(`mention ${String(id)}: no highlight in 10 s`);
        continue;
      }
      await nextFrame();
      const ms = performance.now() - start;
      const kind = el.dataset.kind ?? "none";
      sweep.mentions++;
      sweep.kinds[kind] = (sweep.kinds[kind] ?? 0) + 1;
      if (kind === "page") sweep.pages++;
      const backOk = await backComesBack(el, id);
      const shown = (await until(() => highlightShown(id))) ?? el;
      const highlighted = await highlightedText(shown);
      const sample: ClickSample = {
        mentionId: id,
        fixed,
        kind,
        ms,
        expected: o.raw,
        highlighted,
        correct: sameText(highlighted, o.raw),
        backOk,
      };
      sweep.clicks.push(sample);
    }
  } catch (e) {
    sweep.errors.push(errorMessage(e));
  }
  return sweep;
}

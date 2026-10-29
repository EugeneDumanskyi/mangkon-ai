// The scripted view sweep (`MK_UI_TIMING=1` and `MK_VIEW_SWEEP=1`, NFR-SEC-09): once ingestion
// ends, open the source view of every mention and every linked document once, through the real
// panes, so a network trace of the app covers every view. Not used otherwise.
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionCursor } from "../ipc/bindings/ConnectionCursor";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { ViewSweep } from "../ipc/bindings/ViewSweep";

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
  const sweep: ViewSweep = { mentions: 0, documents: 0, pages: 0, kinds: {}, errors: [] };
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

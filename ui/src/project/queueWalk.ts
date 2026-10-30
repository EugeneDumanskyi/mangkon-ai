// The scripted queue walk (`MK_QUEUE_WALK=1` or `2`, M3c release check): clicks through the
// project and queue controls of the real screens and checks the core's state after each step.
// Only the native dialogs are not clicked: the walk passes their paths to the same commands.
// Not used otherwise.
import type { QueryClient } from "@tanstack/react-query";
import { commands, errorMessage } from "../ipc/commands";
import type { Progress } from "../ipc/bindings/Progress";
import type { QueueWalk } from "../ipc/bindings/QueueWalk";
import type { QueueWalkInfo } from "../ipc/bindings/QueueWalkInfo";

const NAME = "Queue walk";
const ALL = { status: null, fileType: null };

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

type Falsy = false | 0 | "" | null | undefined;

/** `f()` once it is truthy, checked every `every` ms; throws after `ms`. */
async function until<T>(
  what: string,
  f: () => T | Falsy | Promise<T | Falsy>,
  ms = 30_000,
  every = 50,
): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(every);
  }
}

/** Clicks the enabled button labelled `label` inside `root` once there is one. */
async function click(label: string | RegExp, root: () => ParentNode | null = () => document) {
  const matches = (b: HTMLButtonElement) => {
    const text = b.textContent.trim();
    return typeof label === "string" ? text === label : label.test(text);
  };
  const b = await until(`button ${String(label)}`, () =>
    [...(root()?.querySelectorAll("button") ?? [])].find((b) => matches(b) && !b.disabled),
  );
  b.click();
}

/** Sets a select's value the way a user's choice does, so React sees the change. */
function choose(el: HTMLSelectElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set?.call(el, value);
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

function shown(prefix: string): boolean {
  return [...document.querySelectorAll('[role="status"]')].some((el) =>
    el.textContent.startsWith(prefix),
  );
}

const finished = (p: Progress) => p.done + p.failed;
const counts = (p: Progress) =>
  `${p.done.toLocaleString()} done, ${p.failed.toLocaleString()} failed, ` +
  `${p.queued.toLocaleString()} queued, ${p.notProcessed.toLocaleString()} not processed`;
const documents = () => document.querySelector('section[aria-label="Documents"]');

/** Pause in the progress panel; returns the ms from the click until "Paused" shows. */
async function pause(): Promise<number> {
  const t = performance.now();
  await click("Pause");
  await until("Paused in the panel", () => shown("Paused"), 10_000, 10);
  return performance.now() - t;
}

/** Paused and nothing finishing for 3 s. */
async function stillPaused(): Promise<Progress> {
  const a = await commands.progress();
  await sleep(3_000);
  const b = await commands.progress();
  if (!b.paused || b.ingesting) throw new Error(`not paused: ${JSON.stringify(b)}`);
  if (finished(b) !== finished(a)) throw new Error("files finished while paused");
  return b;
}

async function openTab(label: string) {
  await click(label, () => document.querySelector('[role="tablist"]'));
}

async function phase1(info: QueueWalkInfo, client: QueryClient, step: Step) {
  await step("create", async () => {
    const project = await commands.createProject(NAME, info.location);
    client.setQueryData(["project"], project);
    await until("project screen", () => document.querySelector('[role="tablist"]'));
    return project.path;
  });
  await step("add", async () => {
    for (const s of info.sources) await commands.addSource(s);
    const p = await until(
      "Stage A under way",
      async () => {
        const p = await commands.progress();
        return p.stage === "A" && p.ingesting && p.runDone >= 200 ? p : null;
      },
      600_000,
      200,
    );
    return `${p.runDone.toLocaleString()} of ${p.runTotal.toLocaleString()} files read`;
  });
  await step("pause", async () => {
    const ms = await pause();
    const p = await stillPaused();
    return `Paused shown after ${ms.toFixed(0)} ms; ${counts(p)}`;
  });
  await step("resume", async () => {
    const before = finished(await commands.progress());
    await click("Resume");
    const p = await until("files finishing again", async () => {
      const p = await commands.progress();
      return p.ingesting && !p.paused && finished(p) > before + 20 ? p : null;
    });
    return counts(p);
  });
  await step("cancel", async () => {
    await sleep(2_000);
    await click("Cancel");
    await click("Stop reading");
    const p = await until("queue cancelled", async () => {
      const p = await commands.progress();
      return !p.ingesting && p.queued === 0 && p.notProcessed > 0 ? p : null;
    });
    return counts(p);
  });
  await step("not processed in the list", async () => {
    await openTab("Documents");
    const select = await until("status filter", () => documents()?.querySelector("select"));
    choose(select, "not_processed");
    await until("a Not processed row", () =>
      [...(documents()?.querySelectorAll("span") ?? [])].some(
        (s) => s.textContent === "Not processed",
      ),
    ).catch((e: unknown) => {
      const shownText = documents()?.textContent.slice(0, 300) ?? "no list";
      throw new Error(`${errorMessage(e)}; list: ${shownText}`);
    });
    const n = await commands.documentsCount({ status: "not_processed", fileType: null });
    return `${n.toLocaleString()} rows`;
  });
  await step("retry", async () => {
    await click("Retry failed and not processed", documents);
    await until("retry run", async () => (await commands.progress()).ingesting, 10_000);
    const p = await until(
      "retry run done",
      async () => {
        const p = await commands.progress();
        return !p.ingesting && p.queued === 0 ? p : null;
      },
      900_000,
      500,
    );
    if (p.notProcessed !== 0) throw new Error(`still not processed: ${counts(p)}`);
    return counts(p);
  });
  await step("remove", async () => {
    const select = await until("status filter", () => documents()?.querySelector("select"));
    choose(select, "");
    const before = await commands.documentsCount(ALL);
    const box = await until("a row checkbox", () =>
      documents()?.querySelector<HTMLInputElement>('input[type="checkbox"][aria-label^="Select"]'),
    );
    const name = box.getAttribute("aria-label") ?? "";
    box.click();
    await click("Remove from project (1)", documents);
    const dialog = await until("remove confirm", () =>
      documents()?.querySelector('[role="alertdialog"]'),
    );
    const asked = dialog.textContent.replace(/(Remove|Keep)+$/, "").trim();
    await click("Remove", () => documents()?.querySelector('[role="alertdialog"]') ?? null);
    const after = await until("one row fewer", async () => {
      const n = await commands.documentsCount(ALL);
      return n === before - 1 ? n : null;
    });
    return `${name}: "${asked}"; ${before.toLocaleString()} → ${after.toLocaleString()} rows`;
  });
  await step("settings re-process", async () => {
    await openTab("Settings");
    const extra = await until("a second OCR language", () =>
      [...document.querySelectorAll("label")].find(
        (l) => l.querySelector('input[type="checkbox"]') && !l.textContent.startsWith("eng"),
      ),
    );
    const code = extra.textContent.trim();
    extra.querySelector("input")?.click();
    await click("Save");
    await click("Re-process", () => document.querySelector('[role="alertdialog"]'));
    const said = await until("re-process count", () =>
      [...document.querySelectorAll('p[role="status"]')]
        .map((p) => p.textContent)
        .find((t) => t.includes("read again")),
    );
    const p = await until("re-process run", async () => {
      const p = await commands.progress();
      return p.ingesting && p.stage === "A" ? p : null;
    });
    return `added ${code}: "${said}"; ${p.runTotal.toLocaleString()} files in the run`;
  });
  await step("pause before quit", async () => {
    await sleep(1_000);
    const ms = await pause();
    const p = await stillPaused();
    return `Paused shown after ${ms.toFixed(0)} ms; ${counts(p)}`;
  });
}

async function phase2(step: Step) {
  await step("reopen from the recent list", async () => {
    await click(new RegExp(`^${NAME}`), () =>
      document.querySelector('ul[aria-label="Recent projects"]'),
    );
    await until("project screen", () => document.querySelector('[role="tablist"]'));
    await until("Paused in the panel", () => shown("Paused"), 10_000);
    const p = await stillPaused();
    if (p.queued === 0) throw new Error("nothing left in the queue");
    return counts(p);
  });
  await step("resume to the end", async () => {
    await click("Resume");
    const p = await until(
      "run done",
      async () => {
        const p = await commands.progress();
        return !p.ingesting && p.queued === 0 ? p : null;
      },
      1_800_000,
      1_000,
    );
    return counts(p);
  });
}

type Step = (name: string, f: () => Promise<string>) => Promise<void>;

/** Runs one phase and saves it with `record_queue_walk`, which exits the app. */
export async function runQueueWalk(info: QueueWalkInfo, client: QueryClient): Promise<void> {
  const walk: QueueWalk = { phase: info.phase, steps: [], errors: [] };
  const step: Step = async (name, f) => {
    const t = performance.now();
    const detail = await f();
    walk.steps.push({ name, ms: performance.now() - t, detail });
  };
  try {
    if (info.phase === 1) await phase1(info, client, step);
    else await phase2(step);
  } catch (e) {
    walk.errors.push(errorMessage(e));
  }
  await commands.recordQueueWalk(walk);
}

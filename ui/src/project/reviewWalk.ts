// The scripted review walk (`MK_REVIEW_WALK=1`, M7b release check): presses the review queue's keys
// as real `KeyboardEvent`s and clicks the entity page's buttons on the real screens in the release
// app's WebKit, and checks the core after each step. Static-markup tests cannot prove any of this.
// It changes the project it is given, so it is given a copy. Not used otherwise.
import type { QueryClient } from "@tanstack/react-query";
import { commands, errorMessage } from "../ipc/commands";
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { ProposalItem } from "../ipc/bindings/ProposalItem";
import type { ReviewWalk } from "../ipc/bindings/ReviewWalk";
import type { ReviewWalkInfo } from "../ipc/bindings/ReviewWalkInfo";

const EDITED_NAME = "Walk Edited Name";
/** How many proposals the walk uses up. */
const NEEDED = 5;
/** More than a proposal thread run takes on the walk's small project. */
const THREAD_RUN_MS = 3_000;

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

type Falsy = false | 0 | "" | null | undefined;

async function until<T>(
  what: string,
  f: () => T | Falsy | Promise<T | Falsy>,
  ms = 15_000,
): Promise<T> {
  const end = performance.now() + ms;
  for (;;) {
    const v = await f();
    if (v) return v;
    if (performance.now() > end) throw new Error(`timed out waiting for ${what}`);
    await sleep(20);
  }
}

function check(ok: boolean, what: string): void {
  if (!ok) throw new Error(what);
}

/** A key as the keyboard sends it: to the focused element, bubbling up to the handlers. */
function press(key: string, options: KeyboardEventInit = {}): boolean {
  const target = document.activeElement ?? document.body;
  return target.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...options }),
  );
}

const inside = (label: string) => document.querySelector(`section[aria-label="${label}"]`);
const queueIndex = () => Number(inside("Review queue")?.getAttribute("data-index") ?? -1);

async function click(label: string | RegExp, root: () => ParentNode | null = () => document) {
  const matches = (b: HTMLButtonElement) => {
    const texts = [b.textContent.trim(), b.getAttribute("aria-label") ?? ""];
    return texts.some((t) => (typeof label === "string" ? t === label : label.test(t)));
  };
  const b = await until(`button ${String(label)}`, () =>
    [...(root()?.querySelectorAll("button") ?? [])].find((x) => matches(x) && !x.disabled),
  );
  b.click();
}

function setValue(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = el instanceof HTMLInputElement ? HTMLInputElement : HTMLTextAreaElement;
  Object.getOwnPropertyDescriptor(proto.prototype, "value")?.set?.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

function chooseOption(el: HTMLSelectElement, value: string) {
  Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set?.call(el, value);
  el.dispatchEvent(new Event("change", { bubbles: true }));
}

async function openTab(label: RegExp) {
  await click(label, () => document.querySelector('[role="tablist"]'));
}

async function detail(key: string): Promise<EntityDetail> {
  const d = await commands.entityDetail(key);
  if (d === null) throw new Error(`${key} is gone`);
  return d;
}

const proposals = async () => (await commands.proposalsPage(null, null, 100)).rows;
const newest = async () => {
  const row = (await commands.decisionsPage(null, 1)).rows[0];
  if (row === undefined) throw new Error("the log is empty");
  return row;
};

/**
 * After a decision the proposal thread may change the rows, and the held list then offers "Show".
 * Gives the thread time to run, and takes the offer if it comes.
 */
async function showIfOffered() {
  const end = performance.now() + THREAD_RUN_MS;
  while (performance.now() < end) {
    const show = [...(inside("Review queue")?.querySelectorAll("button") ?? [])].find(
      (b) => b.textContent.trim() === "Show",
    );
    if (show !== undefined) {
      show.click();
      await until("the bar gone", () => !show.isConnected);
      return;
    }
    await sleep(20);
  }
}

function cardShows(p: ProposalItem | undefined): boolean {
  const card = inside("Proposal");
  return (
    p !== undefined &&
    (card?.textContent.includes(p.leftName) ?? false) &&
    card?.textContent.includes(p.rightName) === true
  );
}

const CONTROLS = "button, input, select, textarea, a[href], [tabindex]";

/**
 * Every control of the tab can be focused and none is out of the Tab order (a synthetic Tab moves no
 * focus; the hand walk presses the real one). Virtual list rows are one stop, the list itself.
 */
function tabStops(root: Element): { stops: number; names: string[] } {
  const names: string[] = [];
  for (const el of root.querySelectorAll<HTMLElement>(CONTROLS)) {
    if (el.hasAttribute("disabled")) continue;
    const name = el.getAttribute("aria-label") ?? el.textContent.trim().slice(0, 30);
    check(el.tabIndex >= 0, `${name} is out of the Tab order`);
    check(el.tabIndex === 0, `${name} has its own Tab position`);
    el.focus();
    check(document.activeElement === el, `${name} takes no focus`);
    names.push(name);
  }
  return { stops: names.length, names };
}

type Step = (name: string, f: () => Promise<string>) => Promise<void>;

async function queueSteps(step: Step) {
  let start = 0;
  await step("review tab", async () => {
    start = await until("proposals to review", async () => {
      const n = await commands.proposalCount(null);
      return n >= NEEDED ? n : null;
    });
    await openTab(/^Review \(/);
    const first = (await proposals())[0];
    await until("the first proposal on screen", () => cardShows(first));
    const tab = document.querySelector('[role="tablist"]')?.textContent ?? "";
    check(tab.includes(`Review (${start.toLocaleString()})`), `tab says: ${tab}`);
    return `${start.toLocaleString()} proposals, highest score first`;
  });
  await step("side by side (US-11.3 AC1)", async () => {
    const card = await until("the card", () => inside("Proposal"));
    const sides = ["First entity", "Second entity"].map((s) => inside(s));
    for (const side of sides) {
      const text = side?.textContent ?? "";
      for (const part of ["Aliases", "Identifiers", "Evidence"]) {
        check(text.includes(part), `a side has no ${part}: ${text.slice(0, 100)}`);
      }
    }
    check(card.textContent.includes("Score "), "no score");
    const rows = await proposals();
    const p = rows[0];
    check(
      p !== undefined && p.reasons.every((r) => card.textContent.includes(r)),
      "reasons missing",
    );
    const snippets = await until("evidence snippets", () => {
      const n = sides[0]?.querySelectorAll("[data-mention-id]").length ?? 0;
      return n > 0 && n;
    });
    return `${String(snippets)} snippets on the first side`;
  });
  await step("J and K (US-11.3 AC5)", async () => {
    const rows = await proposals();
    check(queueIndex() === 0, `started at ${String(queueIndex())}`);
    press("j");
    await until("the second proposal", () => queueIndex() === 1 && cardShows(rows[1]));
    press("J");
    await until("the third proposal, caps lock", () => queueIndex() === 2 && cardShows(rows[2]));
    press("k");
    await until("back to the second", () => queueIndex() === 1 && cardShows(rows[1]));
    press("k");
    await until("back to the first", () => queueIndex() === 0 && cardShows(rows[0]));
    press("k");
    await sleep(50);
    check(queueIndex() === 0, "moved before the first");
    press("j", { ctrlKey: true });
    await sleep(50);
    check(queueIndex() === 0, "Ctrl+J moved it");
    return "next, previous, stops at the first, a modifier does nothing";
  });
  await step("evidence opens in one click (NFR-USE-04)", async () => {
    const button = await until("a snippet", () =>
      inside("First entity")?.querySelector<HTMLElement>("[data-mention-id]"),
    );
    const id = button.dataset.mentionId ?? "";
    button.click();
    await until("the source at that mention", () =>
      document.querySelector(`section[aria-label="Source"][data-view="m${id}"]`),
    );
    return `mention ${id}`;
  });
  let merged = "";
  await step("A accepts (US-11.3 AC2)", async () => {
    const before = await proposals();
    const p = before[1];
    if (p === undefined) throw new Error("no second proposal");
    press("j");
    await until("the second proposal", () => queueIndex() === 1 && cardShows(p));
    const [l, r] = [await detail(p.leftKey), await detail(p.rightKey)];
    const expected = r.mentionCount > l.mentionCount ? r.name : l.name;
    press("a");
    await until("the merge in the core", async () => {
      const [da, db] = [await commands.entityDetail(p.a), await commands.entityDetail(p.b)];
      return da !== null && da.key === db?.key;
    });
    const d = await detail(p.a);
    check(d.name === expected, `name ${d.name}, expected ${expected}`);
    const loser = expected === l.name ? r.name : l.name;
    check(
      d.aliases.some((a) => a.name === loser),
      `${loser} is no alias`,
    );
    check((await newest()).kind === "accept_merge", "the log has no merge");
    await until("the status line", () =>
      inside("Proposal")?.querySelector('[role="status"]')?.textContent.includes("Merged"),
    );
    check(queueIndex() === 1, `index moved to ${String(queueIndex())}`);
    merged = p.a;
    return `${p.leftName} + ${p.rightName} = ${d.name}, index kept`;
  });
  await step("R rejects (US-11.3 AC3)", async () => {
    await showIfOffered();
    const rows = await proposals();
    const shown = rows[queueIndex()];
    if (shown === undefined) throw new Error("nothing at this index");
    await until("the proposal", () => cardShows(shown));
    press("r");
    await until("the rejection in the log", async () => (await newest()).kind === "reject_merge");
    await until("gone from the queue", async () =>
      (await proposals()).every((p) => p.a !== shown.a || p.b !== shown.b),
    );
    await sleep(2_500); // The proposal thread had time to make it again.
    const again = (await proposals()).some((p) => p.a === shown.a && p.b === shown.b);
    check(!again, "proposed again");
    return `${shown.leftName} / ${shown.rightName}`;
  });
  await step("E then Escape", async () => {
    await showIfOffered();
    const before = (await newest()).seq;
    press("e");
    const form = await until("the edit form", () =>
      document.querySelector('form[aria-label="Edit merge"]'),
    );
    await until("focus in the name field", () => document.activeElement?.tagName === "INPUT");
    press("a"); // Typing in the field is no decision.
    await sleep(50);
    check((await newest()).seq === before, "typing decided");
    press("Escape");
    await until("the form closed", () => !form.isConnected);
    await until("focus back in the queue", () =>
      document.activeElement?.closest('[aria-label="Review queue"]'),
    );
    check((await newest()).seq === before, "Escape decided");
    return "no decision, focus back";
  });
  await step("E, a new name, Enter (US-11.3 AC4)", async () => {
    const shown = (await proposals())[queueIndex()];
    if (shown === undefined) throw new Error("nothing at this index");
    await until("the proposal", () => cardShows(shown));
    press("e");
    const input = await until("the name field", () =>
      document.querySelector<HTMLInputElement>('form[aria-label="Edit merge"] input'),
    );
    await until("focus in the name field", () => document.activeElement === input);
    setValue(input, EDITED_NAME);
    await until("the name typed", () => input.value === EDITED_NAME);
    press("Enter");
    await until("the merge with that name", async () => {
      const d = await commands.entityDetail(shown.a);
      return d?.name === EDITED_NAME;
    });
    return `${shown.leftName} + ${shown.rightName} = ${EDITED_NAME}`;
  });
  await step("every queue control in the Tab order (NFR-USE-06)", async () => {
    await showIfOffered();
    const root = await until("the tab", () => document.querySelector('[role="tabpanel"]'));
    const { stops, names } = tabStops(root);
    for (const name of ["Merge proposals", "Accept A", "Reject R", "Edit E", "Next J"]) {
      check(names.includes(name), `${name} is no stop: ${names.join(", ")}`);
    }
    (root.querySelector('[role="listbox"]') as HTMLElement).focus();
    return `${String(stops)} stops, in document order`;
  });
  return merged;
}

async function filterStep(step: Step) {
  await step("Unreviewed merge opens the filtered queue (US-07.7 AC4)", async () => {
    const filter = {
      kinds: ["person", "org"],
      minSources: 0,
      fileTypes: [],
      find: "",
      sort: "rank" as const,
      descending: false,
      includeIgnored: false,
      includeLow: false,
    };
    const row = await until("an unreviewed row", async () => {
      const page = await commands.connectionsPage(filter, null, 200);
      return page.rows.find((r) => r.unreviewed);
    });
    await openTab(/^Connections$/);
    const find = await until("the find box", () =>
      inside("Connections")?.querySelector<HTMLInputElement>('input[type="search"]'),
    );
    setValue(find, row.value);
    const option = await until("the row", () =>
      [...(inside("Connections")?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])].find(
        (o) => o.textContent.includes(row.value) && o.textContent.includes("Unreviewed merge"),
      ),
    );
    option.click();
    await click(/^Unreviewed merge: review it$/, () => inside("Occurrences"));
    await until("the filter", () =>
      inside("Review queue")?.textContent.includes(`Only ${row.value}`),
    );
    const n = await commands.proposalCount(row.key);
    await until("the filtered list", () =>
      inside("Review queue")?.querySelector("h2")?.textContent.startsWith(`${n.toLocaleString()} `),
    );
    await click("Show all", () => inside("Review queue"));
    await until("the filter gone", () => !inside("Review queue")?.textContent.includes("Only "));
    setValue(find, "");
    return `${row.value}: ${String(n)} of its own`;
  });
}

async function pageSteps(merged: string, step: Step) {
  const entitySection = () => inside("Entity");
  await step("the page has every field (US-12.1 AC1)", async () => {
    await openTab(/^Review \(/);
    await until("a side", () => inside("First entity"));
    await click("Details", () => inside("First entity"));
    const page = await until("the page", entitySection);
    await until("its fields", () =>
      ["Aliases", "Linked identifiers", "Appears in", "Evidence"].every((part) =>
        page.textContent.includes(part),
      ),
    );
    return "name, type, aliases, identifiers, documents, evidence";
  });
  await step("Alt+← leaves the page (US-08.2 AC2)", async () => {
    press("ArrowLeft", { altKey: true });
    await until("the page closed", () => entitySection() === null);
    return "back to where it was";
  });
  await step("evidence and Appears in open the source (US-12.1 AC2)", async () => {
    await openTab(/^Settings$/);
    await until("the decisions list", () => inside("Decisions")?.querySelector("[data-decision]"));
    // Newest first, so the last row is the plain merge of the "A accepts" step.
    const row = await until("the merge's row", () =>
      [...(inside("Decisions")?.querySelectorAll('[data-decision="accept_merge"]') ?? [])].at(-1),
    );
    row.querySelector("button")?.click();
    const page = await until("the page", entitySection);
    const name = (await detail(merged)).name;
    await until("this entity", () => page.querySelector("h2")?.textContent.includes(name));
    const document0 = await until("a document", () =>
      [...page.querySelectorAll("button")].find(
        (b) => b.textContent.includes("mention") && b.title,
      ),
    );
    document0.click();
    await until("the document in the source", () =>
      document.querySelector('section[aria-label="Source"][data-view^="d"]'),
    );
    const evidence = await until("evidence", () =>
      page.querySelector<HTMLElement>("[data-mention-id]"),
    );
    const id = evidence.dataset.mentionId ?? "";
    evidence.click();
    await until("the source at that mention", () =>
      document.querySelector(`section[aria-label="Source"][data-view="m${id}"]`),
    );
    return `a document, then mention ${id}, from the decision list`;
  });
  await step("rename (US-12.2 AC1)", async () => {
    const page = await until("the page", entitySection);
    const input = await until("the name", () => page.querySelector<HTMLInputElement>("form input"));
    setValue(input, "Walk Renamed");
    await click("Rename", entitySection);
    await until("the name in the core", async () => (await detail(merged)).name === "Walk Renamed");
    check((await newest()).kind === "rename", "not logged");
    return "Walk Renamed";
  });
  await step("change type (US-12.2 AC1)", async () => {
    const page = await until("the page", entitySection);
    const select = await until("the type", () => page.querySelector("select"));
    const before = (await detail(merged)).entityType;
    const other = before === "person" ? "org" : "person";
    chooseOption(select, other);
    await until("the type in the core", async () => (await detail(merged)).entityType === other);
    chooseOption(select, before);
    await until("the type back", async () => (await detail(merged)).entityType === before);
    return `${before} → ${other} → ${before}`;
  });
  await step("remove and restore an alias (US-12.2 AC1)", async () => {
    const alias = (await detail(merged)).aliases[0]?.name;
    if (alias === undefined) throw new Error("no alias");
    await click(`Remove alias ${alias}`, entitySection);
    await until("removed in the core", async () =>
      (await detail(merged)).removedAliases.some((a) => a.name === alias),
    );
    await click(`Restore alias ${alias}`, entitySection);
    await until("restored in the core", async () =>
      (await detail(merged)).aliases.some((a) => a.name === alias),
    );
    return alias;
  });
  await step("note (US-12.2 AC2)", async () => {
    const page = await until("the page", entitySection);
    const note = await until("the note", () => page.querySelector("textarea"));
    setValue(note, "Checked by the walk");
    await click("Save note", entitySection);
    await until(
      "the note in the core",
      async () => (await detail(merged)).note === "Checked by the walk",
    );
    return "saved";
  });
  await step("split and put back (US-11.4 AC2, US-11.1 AC2)", async () => {
    const before = await detail(merged);
    const box = await until("a mention to pick", () =>
      entitySection()?.querySelector<HTMLInputElement>('input[aria-label^="Pick "]'),
    );
    box.click();
    await click("Split 1 into a new entity", entitySection);
    await until("the status line", () =>
      entitySection()?.querySelector('[role="status"]')?.textContent.includes("moved to a new"),
    );
    const after = await detail(merged);
    check(after.splits.length === before.splits.length + 1, "the split is not listed");
    check(after.mentionCount === before.mentionCount - 1, "the mention did not move");
    await click("Open", () => entitySection()?.querySelector('[role="status"]') ?? null);
    await until("the new entity's page", () => entitySection()?.textContent.includes("Put back"));
    await click("Put back", entitySection);
    await until("back in the core", async () => {
      const d = await detail(merged);
      return d.splits.length === before.splits.length && d.mentionCount === before.mentionCount;
    });
    return `${before.name}: one mention out and back`;
  });
  await step("undo merge (US-11.4 AC1)", async () => {
    const before = await detail(merged);
    const m = before.merges[0];
    if (m === undefined) throw new Error("no merge");
    await click(/^Undo merge of /, entitySection);
    await until("apart in the core", async () => {
      const [a, b] = [await commands.entityDetail(m.a), await commands.entityDetail(m.b)];
      return a !== null && b !== null && a.key !== b.key;
    });
    const row = await newest();
    check(row.kind === "reject_merge" && row.before === "merged", "not stored as rejected");
    return `${m.aName} and ${m.bName}`;
  });
  await step("the decision list (US-11.5 AC2)", async () => {
    await openTab(/^Settings$/);
    const list = await until("the list", () => inside("Decisions"));
    // The tab shows what it held for a moment, then reads the log again.
    const first = await until("the undo as its newest row", () => {
      const row = list.querySelector("[data-decision]");
      return row?.textContent.includes("Undid merge") === true && row;
    });
    check(/\d/.test(first.textContent), "no time");
    check(first.textContent.includes("Applies"), `no status: ${first.textContent}`);
    const kinds = new Set(
      [...list.querySelectorAll("[data-decision]")].map((r) => r.getAttribute("data-decision")),
    );
    return `kinds on screen: ${[...kinds].join(", ")}`;
  });
}

/** Opens the project, runs the steps and saves them with `record_review_walk`, which exits. */
export async function runReviewWalk(info: ReviewWalkInfo, client: QueryClient): Promise<void> {
  const walk: ReviewWalk = { steps: [], errors: [] };
  const step: Step = async (name, f) => {
    const t = performance.now();
    const detailText = await f();
    walk.steps.push({ name, ms: performance.now() - t, detail: detailText });
  };
  try {
    const project = await commands.openProject(info.project);
    client.setQueryData(["project"], project);
    await until("project screen", () => document.querySelector('[role="tablist"]'));
    const merged = await queueSteps(step);
    await filterStep(step);
    await pageSteps(merged, step);
  } catch (e) {
    walk.errors.push(errorMessage(e));
  }
  await commands.recordReviewWalk(walk);
}

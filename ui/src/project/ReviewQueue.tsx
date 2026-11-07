import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import type { ProposalCursor } from "../ipc/bindings/ProposalCursor";
import type { ProposalItem } from "../ipc/bindings/ProposalItem";
import { PAGE_SIZE } from "../lib/keyset";
import { ENTITY_TYPES, fileName, kindLabel, locationLabel, plural } from "../lib/labels";
import {
  EVIDENCE_PER_SIDE,
  type ReviewAction,
  clampIndex,
  defaultChoice,
  editedChoice,
  knownNames,
  queueUpdated,
  reviewAction,
  scoreText,
} from "../lib/review";
import { useDecision } from "../lib/useDecision";
import { useKeysetList } from "../lib/useKeysetList";
import { Snippet } from "./SearchPanel";

const ROW_HEIGHT = 60;
const IDENTIFIERS_PER_SIDE = 10;
const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
const field = "rounded border border-border bg-transparent px-1 py-0.5 text-sm";

/** One entity of a proposal: names, identifiers, evidence (US-11.3 AC1). */
export function ProposalSide({
  label,
  name,
  detail,
  evidence,
  selected,
  onMention,
  onDetails,
}: {
  label: string;
  name: string;
  detail: EntityDetail | null;
  evidence: Occurrence[];
  selected: number | null;
  onMention: (mentionId: number) => void;
  onDetails: () => void;
}) {
  const identifiers = detail?.linkedIdentifiers.slice(0, IDENTIFIERS_PER_SIDE) ?? [];
  const more = (detail?.linkedTotal ?? 0) - identifiers.length;
  const heading = "mt-2 text-xs font-semibold uppercase text-muted";
  return (
    <section aria-label={label} className="min-w-0 flex-1 p-2">
      <div className="flex items-baseline gap-2">
        <h3 className="min-w-0 truncate font-semibold" title={name}>
          {name}
        </h3>
        <button type="button" className={`${button} ml-auto shrink-0 text-xs`} onClick={onDetails}>
          Details
        </button>
      </div>
      {detail && (
        <p className="text-xs text-muted">
          {kindLabel(detail.entityType)} · {plural(detail.mentionCount, "mention")} ·{" "}
          {plural(detail.documentsTotal, "document")}
        </p>
      )}
      <h4 className={heading}>Aliases</h4>
      {detail === null || detail.aliases.length === 0 ? (
        <p className="text-xs text-muted">None</p>
      ) : (
        <ul className="text-xs">
          {detail.aliases.map((a) => (
            <li key={a.name} className="truncate" title={a.name}>
              {a.name}
            </li>
          ))}
        </ul>
      )}
      <h4 className={heading}>Identifiers</h4>
      {identifiers.length === 0 ? (
        <p className="text-xs text-muted">None</p>
      ) : (
        <ul className="font-mono text-xs">
          {identifiers.map((i) => (
            <li key={i.key} className="truncate" title={i.value}>
              {i.value}
            </li>
          ))}
          {more > 0 && <li className="text-muted">{more.toLocaleString()} more</li>}
        </ul>
      )}
      <h4 className={heading}>Evidence</h4>
      {evidence.length === 0 && <p className="text-xs text-muted">None</p>}
      <ul>
        {evidence.map((o) => {
          const isSelected = o.mentionId === selected;
          return (
            <li key={o.mentionId}>
              <button
                type="button"
                className={`w-full px-1 py-1 text-left ${isSelected ? "bg-accent text-accent-text" : ""}`}
                aria-pressed={isSelected}
                aria-label={`${locationLabel(o.location)} in ${fileName(o.path)}`}
                data-mention-id={o.mentionId}
                onClick={() => {
                  onMention(o.mentionId);
                }}
              >
                <div className={isSelected ? "text-xs" : "text-xs text-muted"}>
                  {fileName(o.path)} · {locationLabel(o.location)}
                </div>
                <Snippet pieces={o.snippet} />
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** "Edit": the final name and type before accepting (US-11.3 AC4). Escape cancels. */
export function MergeEditForm({
  left,
  right,
  busy,
  onAccept,
  onCancel,
}: {
  left: EntityDetail | null;
  right: EntityDetail | null;
  busy: boolean;
  onAccept: (name: string | null, entityType: string | null) => void;
  onCancel: () => void;
}) {
  const defaults = defaultChoice(left, right);
  const [name, setName] = useState(defaults.name);
  const [entityType, setEntityType] = useState(defaults.entityType);
  const names = knownNames(left, right);
  return (
    <form
      aria-label="Edit merge"
      className="flex flex-wrap items-end gap-2 border-b border-border bg-surface-alt px-3 py-2"
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          onCancel();
        } else if (e.key === "Enter" && (e.target as HTMLElement).tagName === "INPUT") {
          e.preventDefault();
          e.currentTarget.requestSubmit();
        }
      }}
      onSubmit={(e) => {
        e.preventDefault();
        if (name.trim() === "") return;
        const edit = editedChoice({ name, entityType }, defaults);
        onAccept(edit.name, edit.entityType);
      }}
    >
      <label className="flex flex-col text-xs">
        Name
        <input
          autoFocus
          className={`${field} w-56`}
          value={name}
          onChange={(e) => {
            setName(e.target.value);
          }}
        />
      </label>
      <label className="flex flex-col text-xs">
        Known names
        <select
          className={`${field} w-48`}
          value={names.includes(name) ? name : ""}
          onChange={(e) => {
            if (e.target.value !== "") setName(e.target.value);
          }}
        >
          <option value="">Pick one…</option>
          {names.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </label>
      <label className="flex flex-col text-xs">
        Type
        <select
          className={field}
          value={entityType}
          onChange={(e) => {
            setEntityType(e.target.value);
          }}
        >
          {ENTITY_TYPES.map((t) => (
            <option key={t} value={t}>
              {kindLabel(t)}
            </option>
          ))}
        </select>
      </label>
      <button type="submit" className={button} disabled={busy || name.trim() === ""}>
        Accept
      </button>
      <button type="button" className={button} onClick={onCancel}>
        Cancel
      </button>
    </form>
  );
}

/** Score, plain reasons and the buttons of the current proposal (US-11.2 AC1, US-11.3). */
export function ProposalHead({
  proposal,
  busy,
  onAction,
}: {
  proposal: ProposalItem;
  busy: boolean;
  onAction: (action: ReviewAction) => void;
}) {
  const buttons: [ReviewAction, string, string][] = [
    ["accept", "Accept", "a"],
    ["reject", "Reject", "r"],
    ["edit", "Edit", "e"],
    ["previous", "Previous", "k"],
    ["next", "Next", "j"],
  ];
  return (
    <div className="border-b border-border px-3 py-2">
      <p className="text-sm">
        <span className="font-semibold">Score {scoreText(proposal.score)}</span>
        {proposal.reasons.map((r) => (
          <span key={r} className="text-muted">
            {" "}
            · {r}
          </span>
        ))}
      </p>
      <div className="mt-1 flex flex-wrap gap-1">
        {buttons.map(([action, label, key]) => (
          <button
            key={action}
            type="button"
            className={button}
            disabled={busy}
            aria-keyshortcuts={key}
            onClick={() => {
              onAction(action);
            }}
          >
            {label} <kbd className="text-xs text-muted">{key.toUpperCase()}</kbd>
          </button>
        ))}
      </div>
    </div>
  );
}

function useSide(key: string | null) {
  const detail = useQuery({
    queryKey: ["entity", key, false],
    queryFn: () => (key === null ? Promise.resolve(null) : commands.entityDetail(key)),
    enabled: key !== null,
  });
  const evidence = useQuery({
    queryKey: ["entity", "evidence", key, EVIDENCE_PER_SIDE, false],
    queryFn: () =>
      key === null ? Promise.resolve([]) : commands.entityEvidence(key, EVIDENCE_PER_SIDE),
    enabled: key !== null,
  });
  return { detail: detail.data ?? null, evidence: evidence.data ?? [] };
}

/** What just happened, with a link to the entity, so a wrong accept is one step from Undo. */
interface Notice {
  text: string;
  open: string;
}

/**
 * The review queue (US-11.3): the proposals, the current one side by side, and the source pane. The
 * list holds still while it is open (US-07.5): only the user's own decisions move it, and a change
 * from the proposal thread only offers "Show".
 */
export function ReviewQueue({
  entity,
  onShowAll,
  index,
  onIndex,
  mention,
  onMention,
  onOpenEntity,
  source,
}: {
  /** The entity the queue is narrowed to (US-07.7 AC4). */
  entity: { key: string; name: string } | null;
  onShowAll: () => void;
  /** Which row is current; kept by the screen, so leaving for a page and returning keeps it. */
  index: number;
  onIndex: (index: number) => void;
  mention: number | null;
  onMention: (mentionId: number) => void;
  onOpenEntity: (key: string) => void;
  source: ReactNode;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const filter = entity?.key ?? null;
  const [lastVisible, setLastVisible] = useState(0);
  const [editing, setEditing] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const list = useKeysetList<ProposalItem, ProposalCursor>({
    queryKey: ["review", filter],
    count: () => commands.proposalCount(filter),
    page: (cursor) => commands.proposalsPage(filter, cursor, PAGE_SIZE),
    lastVisible,
    paused: true,
  });
  // A list on hold keeps what it held, so coming back to the tab reads it again.
  useEffect(() => {
    void list.refresh();
    // Once per list; `list` is a new object each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter]);
  const head = useQuery({
    queryKey: ["review", filter, "head"],
    queryFn: () => commands.proposalsPage(filter, null, 1),
  });
  const decision = useDecision();
  const at = clampIndex(index, list.total);
  const current = list.total > 0 ? list.rowAt(at) : undefined;
  const left = useSide(current?.leftKey ?? null);
  const right = useSide(current?.rightKey ?? null);

  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; nothing here is memoized.
  const virtualizer = useVirtualizer({
    count: list.total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();
  const last = items.at(-1)?.index ?? 0;
  useEffect(() => {
    setLastVisible(Math.max(last, at));
  }, [last, at]);

  const move = (to: number) => {
    const next = clampIndex(to, list.total);
    setEditing(false);
    onIndex(next);
    virtualizer.scrollToIndex(next);
  };
  const toQueue = () => {
    scrollRef.current?.focus();
  };
  const decide = (call: () => Promise<unknown>, text: string, open: string) => {
    setNotice(null);
    decision.mutate(
      async () => {
        // The next proposal takes this one's place; a refused one is gone from the list as well.
        try {
          return await call();
        } finally {
          await list.refresh();
        }
      },
      {
        onSuccess: () => {
          setNotice({ text, open });
          setEditing(false);
          toQueue();
        },
      },
    );
  };
  const accept = (p: ProposalItem, name: string | null, entityType: string | null) => {
    decide(
      () => commands.proposalAccept(p.a, p.b, name, entityType),
      `Merged ${p.leftName} and ${p.rightName}.`,
      p.a,
    );
  };
  const act = (action: ReviewAction) => {
    if (action === "next") {
      move(at + 1);
      return;
    }
    if (action === "previous") {
      move(at - 1);
      return;
    }
    if (current === undefined || decision.isPending) return;
    if (action === "edit") setEditing(true);
    else if (action === "accept") accept(current, null, null);
    else {
      decide(
        () => commands.proposalReject(current.a, current.b),
        `Rejected: ${current.leftName} and ${current.rightName} will not be proposed again.`,
        current.a,
      );
    }
  };

  // On the window, so the keys work wherever the focus is in the tab (US-11.3 AC5).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Behind the AI models screen the tab stays mounted, hidden.
      if (rootRef.current?.offsetParent === null) return;
      if (editing || document.querySelector('[role="alertdialog"]')) return;
      const target = e.target as Partial<HTMLElement> | null;
      const action = reviewAction({
        key: e.key,
        ctrlKey: e.ctrlKey,
        metaKey: e.metaKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        repeat: e.repeat,
        targetTag: target?.tagName ?? "",
        targetEditable: target?.isContentEditable ?? false,
      });
      if (action === null) return;
      e.preventDefault();
      act(action);
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  });

  const behind =
    !decision.isPending &&
    (list.liveTotal !== list.total ||
      (head.dataUpdatedAt >= list.pagesUpdatedAt &&
        queueUpdated(
          { total: list.liveTotal, head: head.data?.rows[0] },
          { total: list.total, head: list.rowAt(0) },
        )));
  const empty = list.total === 0 && head.data !== undefined && head.data.rows.length === 0;

  return (
    <div
      ref={rootRef}
      className="grid h-full min-h-0 grid-cols-[minmax(14rem,1fr)_minmax(24rem,2fr)_minmax(18rem,2fr)]"
    >
      <section
        className="flex min-h-0 flex-col border-r border-border"
        aria-label="Review queue"
        data-index={at}
      >
        <h2 className="border-b border-border px-3 py-2 text-sm font-semibold">
          {plural(list.total, "proposal")}
        </h2>
        {entity !== null && (
          <p className="flex items-center gap-2 border-b border-border px-3 py-1 text-xs">
            <span className="min-w-0 flex-1 truncate">Only {entity.name}</span>
            <button type="button" className="underline" onClick={onShowAll}>
              Show all
            </button>
          </p>
        )}
        {behind && !empty && (
          <div
            role="status"
            className="flex items-center gap-2 border-b border-border bg-accent px-3 py-1 text-sm text-accent-text"
          >
            <span>Queue updated</span>
            <button
              type="button"
              className="rounded border border-current px-2 text-xs"
              onClick={() => {
                void list.refresh();
              }}
            >
              Show
            </button>
          </div>
        )}
        {list.error && (
          <p role="alert" className="px-3 py-2 text-sm">
            {errorMessage(list.error)}
          </p>
        )}
        <div
          ref={scrollRef}
          className="min-h-24 flex-1 overflow-auto focus:outline-2 focus:outline-accent"
          role="listbox"
          aria-label="Merge proposals"
          aria-activedescendant={list.total > 0 ? `proposal-${String(at)}` : undefined}
          tabIndex={0}
          onKeyDown={(e) => {
            const keys: Record<string, number> = {
              ArrowDown: at + 1,
              ArrowUp: at - 1,
              Home: 0,
              End: list.total - 1,
            };
            const to = keys[e.key];
            if (to !== undefined) {
              e.preventDefault();
              move(to);
            }
          }}
        >
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {items.map((item) => {
              const row = list.rowAt(item.index);
              const isCurrent = item.index === at;
              return (
                <div
                  key={item.key}
                  id={`proposal-${String(item.index)}`}
                  role="option"
                  aria-selected={isCurrent}
                  aria-posinset={item.index + 1}
                  aria-setsize={list.total}
                  className={`absolute left-0 top-0 w-full cursor-pointer border-b border-border px-3 py-1.5 text-sm ${
                    isCurrent ? "bg-accent text-accent-text" : ""
                  }`}
                  style={{ height: ROW_HEIGHT, transform: `translateY(${String(item.start)}px)` }}
                  onClick={() => {
                    move(item.index);
                  }}
                >
                  {row ? (
                    <>
                      <div className="truncate" title={`${row.leftName} / ${row.rightName}`}>
                        {row.leftName} / {row.rightName}
                      </div>
                      <div className="text-xs">Score {scoreText(row.score)}</div>
                    </>
                  ) : (
                    <span className="text-muted">…</span>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      </section>
      <section
        className="flex min-h-0 flex-col overflow-auto border-r border-border"
        aria-label="Proposal"
        data-scroll="occurrences"
      >
        {notice !== null && (
          <p
            role="status"
            className="flex items-center gap-2 border-b border-border px-3 py-1 text-sm"
          >
            <span className="min-w-0 flex-1">{notice.text}</span>
            <button
              type="button"
              className="shrink-0 underline"
              onClick={() => {
                onOpenEntity(notice.open);
              }}
            >
              Open
            </button>
          </p>
        )}
        {decision.error && (
          <p role="alert" className="border-b border-border px-3 py-1 text-sm">
            {errorMessage(decision.error)}
          </p>
        )}
        {empty && (
          <p role="status" className="p-3 text-sm text-muted">
            Nothing to review
          </p>
        )}
        {current && (
          <>
            <ProposalHead proposal={current} busy={decision.isPending} onAction={act} />
            {editing && (
              <MergeEditForm
                key={`${current.a}|${current.b}|${String(left.detail !== null && right.detail !== null)}`}
                left={left.detail}
                right={right.detail}
                busy={decision.isPending}
                onAccept={(name, entityType) => {
                  accept(current, name, entityType);
                }}
                onCancel={() => {
                  setEditing(false);
                  toQueue();
                }}
              />
            )}
            <div className="flex min-w-0 divide-x divide-border">
              <ProposalSide
                label="First entity"
                name={current.leftName}
                detail={left.detail}
                evidence={left.evidence}
                selected={mention}
                onMention={onMention}
                onDetails={() => {
                  onOpenEntity(current.leftKey);
                }}
              />
              <ProposalSide
                label="Second entity"
                name={current.rightName}
                detail={right.detail}
                evidence={right.evidence}
                selected={mention}
                onMention={onMention}
                onDetails={() => {
                  onOpenEntity(current.rightKey);
                }}
              />
            </div>
          </>
        )}
      </section>
      {source}
    </div>
  );
}

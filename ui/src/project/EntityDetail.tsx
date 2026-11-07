import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { EntityDetail } from "../ipc/bindings/EntityDetail";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import { entityMentionNote } from "../lib/aiExtraction";
import { groupByDocument } from "../lib/connections";
import { ENTITY_TYPES, fileName, kindLabel, locationLabel, plural } from "../lib/labels";
import { EVIDENCE_ON_PAGE, firstOf } from "../lib/review";
import { useAiExtraction } from "../lib/useAiExtraction";
import { useDecision } from "../lib/useDecision";
import { Snippet } from "./SearchPanel";

const button = "rounded border border-border px-2 py-0.5 text-xs disabled:opacity-50";
const field = "rounded border border-border bg-transparent px-1 py-0.5 text-sm";
const heading = "mt-3 border-b border-border text-xs font-semibold uppercase text-muted";

/** What the page calls on a choice; each is one decision (US-11.5). */
export interface EntityActions {
  rename: (name: string) => void;
  setType: (entityType: string) => void;
  setNote: (note: string) => void;
  removeAlias: (alias: string) => void;
  restoreAlias: (alias: string) => void;
  undoMerge: (a: string, b: string, label: string) => void;
  split: (mentions: number[]) => void;
  putBack: () => void;
}

/** The fields of an entity's page (US-12.1, US-12.2, US-11.4), given what it shows. */
export function EntityPage({
  detail,
  evidence,
  selected,
  includeLow,
  experimental,
  busy,
  actions,
  onMention,
  onDocument,
  onOpenEntity,
  onReview,
  onLow,
}: {
  detail: EntityDetail;
  evidence: Occurrence[];
  selected: number | null;
  includeLow: boolean;
  experimental: boolean;
  busy: boolean;
  actions: EntityActions;
  onMention: (mentionId: number) => void;
  onDocument: (documentId: number) => void;
  onOpenEntity: (key: string) => void;
  onReview: () => void;
  onLow: (on: boolean) => void;
}) {
  const [name, setName] = useState(detail.name);
  const [note, setNote] = useState(detail.note ?? "");
  // What the core has now replaces what was being typed, when it changes.
  const [seen, setSeen] = useState({ key: detail.key, name: detail.name, note: detail.note });
  if (seen.key !== detail.key || seen.name !== detail.name || seen.note !== detail.note) {
    setSeen({ key: detail.key, name: detail.name, note: detail.note });
    setName(detail.name);
    setNote(detail.note ?? "");
  }
  const [picked, setPicked] = useState<number[]>([]);
  const present = picked.filter((id) => evidence.some((o) => o.mentionId === id));
  const toggle = (id: number, on: boolean) => {
    setPicked(on ? [...present, id] : present.filter((p) => p !== id));
  };
  const linkedMore = detail.linkedTotal - detail.linkedIdentifiers.length;
  const documentsNote = firstOf(detail.documents.length, detail.documentsTotal, "documents");
  const evidenceNote = firstOf(evidence.length, detail.evidenceTotal, "mentions");

  return (
    <div className="px-3 pb-3 text-sm">
      {detail.pendingProposals > 0 && (
        <button type="button" className="mt-2 text-xs underline" onClick={onReview}>
          {plural(detail.pendingProposals, "merge proposal")} to review
        </button>
      )}
      {detail.fromTable && <p className="mt-2 text-xs">Name (from table)</p>}

      <form
        className="mt-2 flex items-center gap-1"
        onSubmit={(e) => {
          e.preventDefault();
          actions.rename(name);
        }}
      >
        <label className="flex min-w-0 flex-1 items-center gap-1 text-xs">
          Name
          <input
            className={`${field} min-w-0 flex-1`}
            value={name}
            onChange={(e) => {
              setName(e.target.value);
            }}
          />
        </label>
        <button
          type="submit"
          className={button}
          disabled={busy || name.trim() === "" || name.trim() === detail.name}
        >
          Rename
        </button>
      </form>
      <label className="mt-1 flex items-center gap-1 text-xs">
        Type
        <select
          className={field}
          value={detail.entityType}
          disabled={busy}
          onChange={(e) => {
            actions.setType(e.target.value);
          }}
        >
          {ENTITY_TYPES.map((t) => (
            <option key={t} value={t}>
              {kindLabel(t)}
            </option>
          ))}
        </select>
      </label>

      <h3 className={heading}>Aliases</h3>
      {detail.aliases.length === 0 && <p className="text-xs text-muted">No other names.</p>}
      <ul>
        {detail.aliases.map((a) => (
          <li key={a.name} className="flex items-center gap-2 py-0.5">
            <span className="min-w-0 flex-1 truncate">{a.name}</span>
            <span className="text-xs text-muted">{plural(a.mentions, "mention")}</span>
            <button
              type="button"
              className={button}
              disabled={busy}
              aria-label={`Remove alias ${a.name}`}
              onClick={() => {
                actions.removeAlias(a.name);
              }}
            >
              Remove
            </button>
          </li>
        ))}
        {detail.removedAliases.map((a) => (
          <li key={a.name} className="flex items-center gap-2 py-0.5 text-muted">
            <span className="min-w-0 flex-1 truncate line-through">{a.name}</span>
            <span className="text-xs">{plural(a.mentions, "mention")}</span>
            <button
              type="button"
              className={button}
              disabled={busy}
              aria-label={`Restore alias ${a.name}`}
              onClick={() => {
                actions.restoreAlias(a.name);
              }}
            >
              Restore
            </button>
          </li>
        ))}
      </ul>

      <h3 className={heading}>Note</h3>
      <form
        className="mt-1 flex flex-col items-start gap-1"
        onSubmit={(e) => {
          e.preventDefault();
          actions.setNote(note);
        }}
      >
        <textarea
          aria-label="Note"
          className={`${field} w-full`}
          rows={2}
          value={note}
          onChange={(e) => {
            setNote(e.target.value);
          }}
        />
        <button
          type="submit"
          className={button}
          disabled={busy || note.trim() === (detail.note ?? "")}
        >
          Save note
        </button>
      </form>

      {detail.merges.length > 0 && (
        <>
          <h3 className={heading}>Merged</h3>
          <ul>
            {detail.merges.map((m) => {
              const label = `${m.aName} and ${m.bName}`;
              return (
                <li key={`${m.a}|${m.b}`} className="flex items-center gap-2 py-0.5">
                  <span className="min-w-0 flex-1 truncate" title={label}>
                    {label}
                  </span>
                  <button
                    type="button"
                    className={button}
                    disabled={busy}
                    aria-label={`Undo merge of ${label}`}
                    onClick={() => {
                      actions.undoMerge(m.a, m.b, label);
                    }}
                  >
                    Undo merge
                  </button>
                </li>
              );
            })}
          </ul>
        </>
      )}
      {detail.isSplit && (
        <>
          <h3 className={heading}>Split out</h3>
          <p className="flex flex-wrap items-center gap-2 py-0.5">
            {detail.splitFrom.map((f) => (
              <button
                key={f.key}
                type="button"
                className="underline"
                onClick={() => {
                  onOpenEntity(f.key);
                }}
              >
                From {f.name}
              </button>
            ))}
            <button type="button" className={button} disabled={busy} onClick={actions.putBack}>
              Put back
            </button>
          </p>
        </>
      )}
      {detail.splits.length > 0 && (
        <>
          <h3 className={heading}>Split out of this one</h3>
          <ul>
            {detail.splits.map((s) => (
              <li key={s.key} className="flex items-center gap-2 py-0.5">
                <button
                  type="button"
                  className="min-w-0 flex-1 truncate text-left underline"
                  onClick={() => {
                    onOpenEntity(s.key);
                  }}
                >
                  {s.name}
                </button>
                <span className="text-xs text-muted">{plural(s.mentions, "mention")}</span>
              </li>
            ))}
          </ul>
        </>
      )}

      <h3 className={heading}>Linked identifiers</h3>
      {detail.linkedTotal === 0 && <p className="text-xs text-muted">None found.</p>}
      <ul>
        {detail.linkedIdentifiers.map((i) => (
          <li key={i.key} className="flex items-baseline gap-2 py-0.5">
            <span className="min-w-0 truncate font-mono text-xs" title={i.value}>
              {i.value}
            </span>
            <span className="text-xs text-muted">{kindLabel(i.kind)}</span>
          </li>
        ))}
      </ul>
      {linkedMore > 0 && <p className="text-xs text-muted">{linkedMore.toLocaleString()} more</p>}

      <h3 className={heading}>Appears in {plural(detail.documentsTotal, "document")}</h3>
      {documentsNote !== null && <p className="text-xs text-muted">{documentsNote}</p>}
      <ul>
        {detail.documents.map((d) => (
          <li key={d.documentId}>
            <button
              type="button"
              className="flex w-full items-baseline gap-2 py-0.5 text-left"
              title={d.path ?? undefined}
              onClick={() => {
                onDocument(d.documentId);
              }}
            >
              <span className="min-w-0 flex-1 truncate">{fileName(d.path)}</span>
              <span className="rounded border border-current px-1 text-xs uppercase">
                {d.fileType}
              </span>
              <span className="text-xs text-muted">{plural(d.mentions, "mention")}</span>
            </button>
          </li>
        ))}
      </ul>

      <h3 className={heading}>Evidence ({detail.evidenceTotal.toLocaleString()})</h3>
      <div className="flex flex-wrap items-center gap-2 py-1 text-xs">
        <label className="flex items-center gap-1">
          <input
            type="checkbox"
            checked={includeLow}
            onChange={(e) => {
              onLow(e.target.checked);
            }}
          />
          Show low confidence
        </label>
        <button
          type="button"
          className={button}
          disabled={busy || present.length === 0}
          onClick={() => {
            actions.split(present);
            setPicked([]);
          }}
        >
          Split {present.length.toLocaleString()} into a new entity
        </button>
      </div>
      {evidenceNote !== null && <p className="text-xs text-muted">{evidenceNote}</p>}
      {groupByDocument(evidence).map((g) => (
        <section key={g.documentId} aria-label={`Evidence in ${fileName(g.path)}`}>
          <h4 className="truncate bg-surface py-0.5 text-xs font-semibold" title={g.path ?? ""}>
            {fileName(g.path)}
          </h4>
          <ul>
            {g.occurrences.map((o) => {
              const place = locationLabel(o.location);
              const isSelected = o.mentionId === selected;
              return (
                <li key={o.mentionId} className="flex items-start gap-1">
                  <input
                    type="checkbox"
                    className="mt-2"
                    aria-label={`Pick ${place} in ${fileName(o.path)}`}
                    checked={present.includes(o.mentionId)}
                    onChange={(e) => {
                      toggle(o.mentionId, e.target.checked);
                    }}
                  />
                  <button
                    type="button"
                    className={`min-w-0 flex-1 px-1 py-1 text-left ${
                      isSelected ? "bg-accent text-accent-text" : ""
                    }`}
                    aria-pressed={isSelected}
                    data-mention-id={o.mentionId}
                    onClick={() => {
                      onMention(o.mentionId);
                    }}
                  >
                    <div className={isSelected ? "text-xs" : "text-xs text-muted"}>
                      {place}
                      {o.lowConfidence ? " · low confidence" : ""}
                      {entityMentionNote(o, experimental)}
                    </div>
                    <Snippet pieces={o.snippet} />
                  </button>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </div>
  );
}

/** What just happened, with a link to the entity when there is one to open. */
interface Notice {
  text: string;
  open?: string;
}

/** An entity's page in place of the occurrences (US-12.1), reading and deciding through the core. */
export function EntityPanel({
  entityKey,
  selected,
  onMention,
  onDocument,
  onOpenEntity,
  onClose,
  onReview,
}: {
  entityKey: string;
  selected: number | null;
  onMention: (mentionId: number) => void;
  onDocument: (documentId: number) => void;
  onOpenEntity: (key: string) => void;
  onClose: () => void;
  onReview: (key: string, name: string) => void;
}) {
  const [showLow, setShowLow] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const page = useQuery({
    queryKey: ["entity", entityKey, showLow],
    queryFn: () => commands.entityDetail(entityKey, showLow),
  });
  const evidence = useQuery({
    queryKey: ["entity", "evidence", entityKey, EVIDENCE_ON_PAGE, showLow],
    queryFn: () => commands.entityEvidence(entityKey, EVIDENCE_ON_PAGE, showLow),
  });
  const ai = useAiExtraction();
  const decision = useDecision();
  const detail = page.data ?? null;
  const key = detail?.key ?? entityKey;

  const decide = (call: () => Promise<unknown>, text: string) => {
    setNotice(null);
    decision.mutate(call, {
      onSuccess: () => {
        setNotice({ text });
      },
    });
  };
  const actions: EntityActions = {
    rename: (name) => {
      decide(() => commands.entityRename(key, name), `Renamed to ${name.trim()}`);
    },
    setType: (entityType) => {
      decide(() => commands.entitySetType(key, entityType), `Type: ${kindLabel(entityType)}`);
    },
    setNote: (note) => {
      decide(
        () => commands.entitySetNote(key, note),
        note.trim() === "" ? "Note cleared" : "Note saved",
      );
    },
    removeAlias: (alias) => {
      decide(() => commands.aliasRemove(key, alias), `Removed alias ${alias}`);
    },
    restoreAlias: (alias) => {
      decide(() => commands.aliasRestore(key, alias), `Restored alias ${alias}`);
    },
    undoMerge: (a, b, label) => {
      decide(
        () => commands.mergeUndo(a, b),
        `Undid the merge of ${label}. It will not be proposed again.`,
      );
    },
    split: (mentions) => {
      setNotice(null);
      decision.mutate(() => commands.entitySplit(key, mentions), {
        onSuccess: (made) => {
          setNotice({
            text: `${plural(mentions.length, "mention")} moved to a new entity.`,
            open: (made as { key: string }).key,
          });
        },
      });
    },
    putBack: () => {
      const back = detail?.splitFrom[0]?.key;
      decision.mutate(() => commands.splitUndo(key), {
        onSuccess: () => {
          if (back === undefined) onClose();
          else onOpenEntity(back);
        },
      });
    },
  };

  const error = page.error ?? evidence.error ?? decision.error;
  return (
    <section className="flex min-h-0 flex-col border-r border-border" aria-label="Entity">
      <div className="flex items-baseline gap-2 border-b border-border px-3 py-2 text-sm">
        <h2 className="min-w-0 truncate font-semibold">{detail?.name ?? "…"}</h2>
        {detail && (
          <span className="shrink-0 text-xs text-muted">
            {kindLabel(detail.entityType)} · {plural(detail.mentionCount, "mention")}
          </span>
        )}
        <button type="button" className={`${button} ml-auto shrink-0`} onClick={onClose}>
          Close
        </button>
      </div>
      {notice !== null && (
        <p
          role="status"
          className="flex items-center gap-2 border-b border-border px-3 py-1 text-xs"
        >
          <span className="min-w-0 flex-1">{notice.text}</span>
          {notice.open !== undefined && (
            <button
              type="button"
              className="underline"
              onClick={() => {
                if (notice.open !== undefined) onOpenEntity(notice.open);
              }}
            >
              Open
            </button>
          )}
        </p>
      )}
      {error && (
        <p role="alert" className="border-b border-border px-3 py-1 text-sm">
          {errorMessage(error)}
        </p>
      )}
      {page.isSuccess && detail === null && (
        <p role="status" className="px-3 py-2 text-sm text-muted">
          This person or organization is no longer in the project.
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-auto" data-scroll="occurrences">
        {detail && (
          <EntityPage
            detail={detail}
            evidence={evidence.data ?? []}
            selected={selected}
            includeLow={showLow}
            experimental={ai.data?.experimental ?? false}
            busy={decision.isPending}
            actions={actions}
            onMention={onMention}
            onDocument={onDocument}
            onOpenEntity={onOpenEntity}
            onReview={() => {
              onReview(detail.key, detail.name);
            }}
            onLow={setShowLow}
          />
        )}
      </div>
    </section>
  );
}

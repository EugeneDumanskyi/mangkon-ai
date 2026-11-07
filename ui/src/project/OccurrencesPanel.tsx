import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import { aiMentionNote, entityMentionNote } from "../lib/aiExtraction";
import { groupByDocument } from "../lib/connections";
import { fileName, locationLabel, plural } from "../lib/labels";
import { useAiExtraction } from "../lib/useAiExtraction";
import { Snippet } from "./SearchPanel";

/** Where one connection's identifier was found (US-07.3 AC2): one click opens the spot. */
export function OccurrencesPanel({
  connection,
  selected,
  onSelect,
  onDetails,
  onReview,
}: {
  connection: ConnectionItem | null;
  selected: number | null;
  onSelect: (mentionId: number) => void;
  /** Opens the page of the entity (US-12.1). */
  onDetails: (key: string) => void;
  /** Opens the review queue for the entity (US-07.7 AC4). */
  onReview: (connection: ConnectionItem) => void;
}) {
  const listRef = useRef<HTMLDivElement>(null);
  const key = connection?.key ?? null;
  const isName = connection?.kind === "person" || connection?.kind === "org";
  // A connection only low-confidence mentions make has nothing to show without them.
  const [showLow, setShowLow] = useState(false);
  const includeLow = isName && (showLow || connection.lowOnly);
  const occurrences = useQuery({
    queryKey: ["occurrences", key, includeLow],
    queryFn: () => (key === null ? Promise.resolve([]) : commands.occurrences(key, includeLow)),
    enabled: key !== null,
  });
  const ai = useAiExtraction();
  const experimental = ai.data?.experimental ?? false;

  // Keep the shown mention's row in view as Next / Previous move it (US-08.2 AC1).
  useEffect(() => {
    if (selected === null) return;
    listRef.current
      ?.querySelector(`[data-mention-id="${String(selected)}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [selected, occurrences.data]);

  if (connection === null) {
    return (
      <section className="border-r border-border p-3 text-sm text-muted" aria-label="Occurrences">
        Pick a connection to see where it was found.
      </section>
    );
  }
  const rows = occurrences.data ?? [];
  const focusSibling = (from: HTMLElement, step: number) => {
    const buttons = [...(listRef.current?.querySelectorAll("button") ?? [])];
    buttons[buttons.indexOf(from as HTMLButtonElement) + step]?.focus();
  };

  return (
    <section className="flex min-h-0 flex-col border-r border-border" aria-label="Occurrences">
      <h2 className="border-b border-border px-3 py-2 text-sm">
        <span className="font-mono font-semibold">{connection.value}</span>
        <span className="text-muted"> · {plural(rows.length, "mention")}</span>
      </h2>
      {isName && (
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-3 py-1 text-xs">
          <button
            type="button"
            className="rounded border border-border px-2 py-0.5"
            onClick={() => {
              onDetails(connection.key);
            }}
          >
            Details
          </button>
          {connection.unreviewed && (
            <button
              type="button"
              className="rounded px-1 underline"
              onClick={() => {
                onReview(connection);
              }}
            >
              Unreviewed merge: review it
            </button>
          )}
        </div>
      )}
      {isName && (
        <label className="flex items-center gap-1 border-b border-border px-3 py-1 text-xs">
          <input
            type="checkbox"
            checked={includeLow}
            disabled={connection.lowOnly}
            onChange={(e) => {
              setShowLow(e.target.checked);
            }}
          />
          Show low confidence
        </label>
      )}
      {occurrences.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(occurrences.error)}
        </p>
      )}
      <div ref={listRef} className="min-h-0 flex-1 overflow-auto" data-scroll="occurrences">
        {groupByDocument(rows).map((g) => (
          <section
            key={g.documentId}
            aria-label={`In ${fileName(g.path)}`}
            className="border-b border-border"
          >
            <h3
              className="flex items-baseline gap-2 bg-surface px-3 py-1 text-sm"
              title={g.path ?? undefined}
            >
              <span className="truncate font-semibold">{fileName(g.path)}</span>
              <span className="shrink-0 rounded border border-current px-1 text-xs uppercase">
                {g.fileType}
              </span>
              <span className="shrink-0 text-xs text-muted">
                {plural(g.occurrences.length, "mention")}
              </span>
            </h3>
            <ul>
              {g.occurrences.map((o) => {
                const isSelected = o.mentionId === selected;
                return (
                  <li key={o.mentionId}>
                    <button
                      type="button"
                      className={`w-full px-3 py-1.5 text-left text-sm focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent ${
                        isSelected ? "bg-accent text-accent-text" : ""
                      }`}
                      aria-pressed={isSelected}
                      aria-label={`${locationLabel(o.location)} in ${fileName(o.path)}`}
                      title={o.path ?? undefined}
                      data-mention-id={o.mentionId}
                      onClick={() => {
                        onSelect(o.mentionId);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                          e.preventDefault();
                          focusSibling(e.currentTarget, e.key === "ArrowDown" ? 1 : -1);
                        }
                      }}
                    >
                      <div className={isSelected ? "text-xs" : "text-xs text-muted"}>
                        {locationLabel(o.location)}
                        {o.lowConfidence ? " · low confidence" : ""}
                        {isName
                          ? entityMentionNote(o, experimental)
                          : aiMentionNote(o, experimental)}
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
    </section>
  );
}

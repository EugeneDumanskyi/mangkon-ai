import { useQuery } from "@tanstack/react-query";
import { useRef } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import { fileName, locationLabel, plural } from "../lib/labels";

/** Where one connection's identifier was found (US-07.3 AC2): one click opens the spot. */
export function OccurrencesPanel({
  connection,
  selected,
  onSelect,
}: {
  connection: ConnectionItem | null;
  selected: number | null;
  onSelect: (mentionId: number) => void;
}) {
  const listRef = useRef<HTMLUListElement>(null);
  const key = connection?.key ?? null;
  const occurrences = useQuery({
    queryKey: ["occurrences", key],
    queryFn: () => (key === null ? Promise.resolve([]) : commands.occurrences(key)),
    enabled: key !== null,
  });

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
      {occurrences.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(occurrences.error)}
        </p>
      )}
      <ul ref={listRef} className="min-h-0 flex-1 overflow-auto">
        {rows.map((o) => {
          const isSelected = o.mentionId === selected;
          return (
            <li key={o.mentionId} className="border-b border-border">
              <button
                type="button"
                className={`w-full px-3 py-1.5 text-left text-sm focus:outline-2 focus:-outline-offset-2 focus:outline-accent ${
                  isSelected ? "bg-accent text-accent-text" : ""
                }`}
                aria-pressed={isSelected}
                title={o.path ?? undefined}
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
                <div className="truncate">
                  {fileName(o.path)} <span className="uppercase">({o.fileType})</span>
                </div>
                <div className={isSelected ? "text-xs" : "text-xs text-muted"}>
                  {locationLabel(o.location)}
                  {o.lowConfidence ? " · low confidence" : ""}
                </div>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

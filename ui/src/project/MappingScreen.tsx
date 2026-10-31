import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { MappingView } from "../ipc/bindings/MappingView";
import { COLUMN_TYPE_CHOICES, columnTypeLabel, fileName, mappingStateLabel } from "../lib/labels";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/**
 * The column types of one file: per column its header, up to 5 sample values and a type,
 * marked "Suggested" until confirmed (US-05.1 AC1); Confirm applies them, and a file with the
 * same headers can lend its mapping (US-05.2 AC1/AC2).
 */
export function MappingForm({
  view,
  busy,
  onConfirm,
  onReuse,
}: {
  view: MappingView;
  busy: boolean;
  onConfirm: (types: Record<string, string>) => void;
  onReuse: (fromRefId: number) => void;
}) {
  const [types, setTypes] = useState<Record<string, string>>(() =>
    Object.fromEntries(view.columns.map((c) => [c.key, c.assigned ?? c.suggested])),
  );
  const hidden = view.columnCount - view.columns.length;
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        onConfirm(types);
      }}
    >
      {view.reuse.map((r) => (
        <div key={r.refId}>
          <button
            type="button"
            className={button}
            disabled={busy}
            onClick={() => {
              onReuse(r.refId);
            }}
          >
            Use the same mapping as {fileName(r.path)}
          </button>
        </div>
      ))}
      <table className="text-sm">
        <thead>
          <tr className="text-left text-muted">
            <th className="pr-3 font-normal">Column</th>
            <th className="pr-3 font-normal">Sample values</th>
            <th className="font-normal">Type</th>
          </tr>
        </thead>
        <tbody>
          {view.columns.map((c) => (
            <tr key={c.key} className="border-t border-border align-top">
              <td className="py-1 pr-3 font-mono">{c.header}</td>
              <td className="max-w-md py-1 pr-3">
                {c.samples.map((s, i) => (
                  <span key={i} className="block truncate text-muted" title={s}>
                    {s}
                  </span>
                ))}
              </td>
              <td className="py-1">
                <span className="flex items-center gap-2">
                  <select
                    aria-label={`Type of ${c.header}`}
                    className="rounded border border-border bg-surface px-1 text-sm"
                    value={types[c.key]}
                    onChange={(e) => {
                      setTypes({ ...types, [c.key]: e.target.value });
                    }}
                  >
                    {COLUMN_TYPE_CHOICES.map((t) => (
                      <option key={t} value={t}>
                        {columnTypeLabel(t)}
                      </option>
                    ))}
                  </select>
                  {c.assigned === null && types[c.key] === c.suggested && (
                    <span className="rounded border border-current px-1 text-xs">Suggested</span>
                  )}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {hidden > 0 && (
        <p className="text-sm text-muted">
          {hidden.toLocaleString()} more columns are not shown and stay ignored.
        </p>
      )}
      <div>
        <button type="submit" className={button} disabled={busy}>
          Confirm
        </button>
      </div>
    </form>
  );
}

/** "Map columns" of a CSV or JSON file, below the documents list. */
export function MappingScreen({ refId, onClose }: { refId: number; onClose: () => void }) {
  const client = useQueryClient();
  const view = useQuery({
    queryKey: ["mapping", refId],
    queryFn: () => commands.mappingView(refId),
  });
  const done = () => {
    void client.invalidateQueries({ queryKey: ["mapping", refId] });
    void client.invalidateQueries({ queryKey: ["documents"] });
    void client.invalidateQueries({ queryKey: ["connections"] });
    onClose();
  };
  const confirm = useMutation({
    mutationFn: (types: Record<string, string>) =>
      commands.confirmMapping(
        refId,
        Object.entries(types).map(([key, columnType]) => ({ key, columnType })),
      ),
    onSuccess: done,
  });
  const reuse = useMutation({
    mutationFn: (from: number) => commands.reuseMapping(refId, from),
    onSuccess: done,
  });
  const error = view.error ?? confirm.error ?? reuse.error;
  return (
    <section
      aria-label="Map columns"
      className="flex max-h-[60%] flex-col gap-2 overflow-auto border-t border-border bg-surface-alt p-3"
    >
      <div className="flex items-baseline gap-2">
        <h3 className="min-w-0 flex-1 truncate font-semibold">
          Map columns{view.data ? ` · ${fileName(view.data.path)}` : ""}
        </h3>
        {view.data && (
          <span className="rounded border border-current px-1 text-xs">
            Columns: {mappingStateLabel(view.data.state)}
          </span>
        )}
        <button type="button" className={button} onClick={onClose}>
          Close
        </button>
      </div>
      {view.data?.state === "needs_review" && (
        <p className="text-sm">
          The headers changed since the mapping was confirmed. Unchanged columns keep their type.
        </p>
      )}
      {error && (
        <p role="alert" className="text-sm">
          {errorMessage(error)}
        </p>
      )}
      {view.data === null && <p className="text-sm text-muted">This file has no columns to map.</p>}
      {view.data && (
        <MappingForm
          key={JSON.stringify(view.data)}
          view={view.data}
          busy={confirm.isPending || reuse.isPending}
          onConfirm={(types) => {
            confirm.mutate(types);
          }}
          onReuse={(from) => {
            reuse.mutate(from);
          }}
        />
      )}
    </section>
  );
}

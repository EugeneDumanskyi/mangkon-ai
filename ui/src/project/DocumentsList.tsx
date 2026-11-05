import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { DocumentCursor } from "../ipc/bindings/DocumentCursor";
import type { DocumentFilter } from "../ipc/bindings/DocumentFilter";
import type { DocumentItem } from "../ipc/bindings/DocumentItem";
import { PAGE_SIZE } from "../lib/keyset";
import { fileName, mappingStateLabel, plural } from "../lib/labels";
import { FindPeople } from "./AiExtraction";
import { MappingScreen } from "./MappingScreen";
import { STATUS_LABELS, canRetry, sizeLabel, statusLabel, unitsLabel } from "../lib/progress";
import { useKeysetList } from "../lib/useKeysetList";

const ROW_HEIGHT = 52;
const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** One row of the documents list (US-02.8 AC1). */
export function DocumentRowView({
  row,
  checked,
  onCheck,
  onDetails,
  onMap,
}: {
  row: DocumentItem;
  checked: boolean;
  onCheck: (checked: boolean) => void;
  onDetails: () => void;
  /** Opens "Map columns" of a CSV or JSON file (US-05.1). */
  onMap?: () => void;
}) {
  const name = fileName(row.path);
  return (
    <div className="flex h-full items-center gap-3">
      <input
        type="checkbox"
        aria-label={`Select ${name}`}
        checked={checked}
        onChange={(e) => {
          onCheck(e.target.checked);
        }}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="truncate">{name}</span>
          {row.otherPaths > 0 && (
            <span className="shrink-0 text-xs text-muted">
              same content as {plural(row.otherPaths, "other path")}
            </span>
          )}
          {row.cardLikeCount > 0 && (
            <span
              className="shrink-0 rounded border border-current px-1 text-xs"
              title={`Card-like numbers: ${row.cardLikeCount.toLocaleString()}. Shown masked; never a connection.`}
            >
              Contains card-like number
            </span>
          )}
          {row.mappingState !== null && (
            <span className="shrink-0 rounded border border-current px-1 text-xs">
              Columns: {mappingStateLabel(row.mappingState)}
            </span>
          )}
        </div>
        <div className="truncate font-mono text-xs text-muted" title={row.path}>
          {row.path}
        </div>
      </div>
      <span className="w-12 shrink-0 text-xs uppercase text-muted">{row.fileType}</span>
      <span className="w-20 shrink-0 text-right text-xs text-muted">{sizeLabel(row.size)}</span>
      <span className="w-24 shrink-0 text-right text-xs text-muted">
        {unitsLabel(row.units, row.unit)}
      </span>
      <span className="w-48 shrink-0 text-sm">
        <span className="block truncate">
          {statusLabel(row.status)}
          {row.warnings.length > 0 && ` · ${plural(row.warnings.length, "warning")}`}
        </span>
        {row.failureReason !== null && (
          <span className="block truncate text-xs text-muted" title={row.failureReason}>
            {row.failureReason}
          </span>
        )}
      </span>
      {row.mappingState !== null && onMap && (
        <button type="button" className={`${button} shrink-0`} onClick={onMap}>
          Map columns
        </button>
      )}
      {(canRetry(row.status) || row.warnings.length > 0) && (
        <button type="button" className={`${button} shrink-0`} onClick={onDetails}>
          Show details
        </button>
      )}
    </div>
  );
}

/** "Show details" of a failed or not-processed file, with Retry (US-02.7 AC2–AC3). */
export function DocumentDetails({
  row,
  onRetry,
  onClose,
  retrying,
}: {
  row: DocumentItem;
  onRetry: () => void;
  onClose: () => void;
  retrying: boolean;
}) {
  return (
    <section
      aria-label="File details"
      className="flex flex-col gap-1 border-t border-border bg-surface-alt p-3 text-sm"
    >
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate font-mono" title={row.path}>
          {row.path}
        </span>
        {canRetry(row.status) && (
          <button type="button" className={button} disabled={retrying} onClick={onRetry}>
            Retry
          </button>
        )}
        <button type="button" className={button} onClick={onClose}>
          Close
        </button>
      </div>
      <p>
        {statusLabel(row.status)}
        {row.failureReason !== null && `: ${row.failureReason}`}
        {row.status === "not_processed" && ": reading was cancelled before this file finished."}
      </p>
      {row.warnings.map((w) => (
        <p key={w} className="text-muted">
          Warning: {w}
        </p>
      ))}
      {row.failureDetail !== null && (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap rounded border border-border bg-surface p-2 font-mono text-xs">
          {row.failureDetail}
        </pre>
      )}
    </section>
  );
}

/** The files of the project, filtered by status and type (US-02.8), with retry and remove. */
export function DocumentsList() {
  const client = useQueryClient();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [filter, setFilter] = useState<DocumentFilter>({ status: null, fileType: null });
  const [lastVisible, setLastVisible] = useState(0);
  const [selected, setSelected] = useState<Map<number, DocumentItem>>(new Map());
  const [details, setDetails] = useState<DocumentItem | null>(null);
  const [mapping, setMapping] = useState<number | null>(null);
  const [impact, setImpact] = useState<number | null>(null);
  const types = useQuery({ queryKey: ["documents", "types"], queryFn: commands.documentTypes });
  const list = useKeysetList<DocumentItem, DocumentCursor>({
    queryKey: ["documents", filter],
    count: () => commands.documentsCount(filter),
    page: (cursor) => commands.documentsPage(filter, cursor, PAGE_SIZE),
    lastVisible,
  });
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
    setLastVisible(last);
  }, [last]);

  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["documents"] });
    void client.invalidateQueries({ queryKey: ["progress"] });
  };
  const retry = useMutation({
    mutationFn: commands.retry,
    onSuccess: () => {
      setDetails(null);
      refresh();
    },
  });
  const askImpact = useMutation({
    mutationFn: () => commands.removalImpact([...selected.keys()]),
    onSuccess: setImpact,
  });
  const remove = useMutation({
    mutationFn: () => commands.removeFiles([...selected.keys()]),
    onSuccess: () => {
      setSelected(new Map());
      setImpact(null);
      setDetails(null);
      refresh();
      void client.invalidateQueries({ queryKey: ["connections"] });
    },
  });
  const error = list.error ?? retry.error ?? askImpact.error ?? remove.error;
  const select = "rounded border border-border bg-surface px-1 py-0.5 text-sm";

  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="Documents">
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-3 py-2 text-sm">
        <span className="font-semibold">{plural(list.total, "file")}</span>
        <label className="flex items-center gap-1">
          Status
          <select
            className={select}
            value={filter.status ?? ""}
            onChange={(e) => {
              setFilter({ ...filter, status: e.target.value || null });
            }}
          >
            <option value="">All</option>
            {Object.entries(STATUS_LABELS).map(([value, label]) => (
              <option key={value} value={value}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-1">
          Type
          <select
            className={select}
            value={filter.fileType ?? ""}
            onChange={(e) => {
              setFilter({ ...filter, fileType: e.target.value || null });
            }}
          >
            <option value="">All</option>
            {(types.data ?? []).map((t) => (
              <option key={t.fileType} value={t.fileType}>
                {t.fileType} ({t.count.toLocaleString()})
              </option>
            ))}
          </select>
        </label>
        <button
          type="button"
          className={button}
          disabled={retry.isPending}
          onClick={() => {
            retry.mutate(null);
          }}
        >
          Retry failed and not processed
        </button>
        <FindPeople contentIds={[...new Set([...selected.values()].map((r) => r.contentId))]} />
        {selected.size > 0 &&
          (impact === null ? (
            <button
              type="button"
              className={button}
              disabled={askImpact.isPending}
              onClick={() => {
                askImpact.mutate();
              }}
            >
              Remove from project ({selected.size.toLocaleString()})
            </button>
          ) : (
            <span role="alertdialog" className="flex flex-wrap items-center gap-2">
              Remove {plural(selected.size, "file")} from the project?{" "}
              {impact === 0
                ? "No connection loses evidence."
                : `${plural(impact, "connection")} lose${impact === 1 ? "s" : ""} evidence.`}{" "}
              The files on disk are not touched.
              <button
                type="button"
                className={button}
                disabled={remove.isPending}
                onClick={() => {
                  remove.mutate();
                }}
              >
                Remove
              </button>
              <button
                type="button"
                className={button}
                onClick={() => {
                  setImpact(null);
                }}
              >
                Keep
              </button>
            </span>
          ))}
      </div>
      {error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(error)}
        </p>
      )}
      {list.total === 0 && !list.error && (
        <p className="px-3 py-2 text-sm text-muted">No files match.</p>
      )}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {items.map((item) => {
            const row = list.rowAt(item.index);
            return (
              <div
                key={item.key}
                className="absolute left-0 top-0 w-full border-b border-border px-3"
                style={{ height: ROW_HEIGHT, transform: `translateY(${String(item.start)}px)` }}
              >
                {row ? (
                  <DocumentRowView
                    row={row}
                    checked={selected.has(row.refId)}
                    onCheck={(on) => {
                      const next = new Map(selected);
                      if (on) next.set(row.refId, row);
                      else next.delete(row.refId);
                      setSelected(next);
                      setImpact(null);
                    }}
                    onDetails={() => {
                      setMapping(null);
                      setDetails(row);
                    }}
                    onMap={() => {
                      setDetails(null);
                      setMapping(row.refId);
                    }}
                  />
                ) : (
                  <span className="text-muted">…</span>
                )}
              </div>
            );
          })}
        </div>
      </div>
      {mapping !== null && (
        <MappingScreen
          key={mapping}
          refId={mapping}
          onClose={() => {
            setMapping(null);
          }}
        />
      )}
      {details !== null && (
        <DocumentDetails
          row={details}
          retrying={retry.isPending}
          onRetry={() => {
            retry.mutate([details.contentId]);
          }}
          onClose={() => {
            setDetails(null);
          }}
        />
      )}
    </section>
  );
}

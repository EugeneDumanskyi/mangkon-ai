import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { MeaningStatus } from "../ipc/bindings/MeaningStatus";
import type { SearchHit } from "../ipc/bindings/SearchHit";
import type { SegmentHit } from "../ipc/bindings/SegmentHit";
import { aboutLabel } from "../lib/aiExtraction";
import { plural } from "../lib/labels";
import { meaningCanStart, meaningRowShown, meaningStateLabel } from "../lib/meaning";
import { etaLabel } from "../lib/progress";
import { useMeaning } from "../lib/useMeaning";
import { SearchResultRow } from "./SearchPanel";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
/** A model's absence is fixed elsewhere: where, in the same words as Settings. */
const NO_MODEL = "No model allowed for this project";

function useRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: ["progress"] });
}

function Failure({ error }: { error: unknown }) {
  if (error === null || error === undefined) return null;
  return (
    <span role="alert" className="text-sm">
      {errorMessage(error)}
    </span>
  );
}

/** Starts what has not been embedded yet, with what it costs when that is known. */
function StartAction({ info }: { info: MeaningStatus }) {
  const refresh = useRefresh();
  const start = useMutation({ mutationFn: commands.meaningRun, onSettled: () => void refresh() });
  return (
    <>
      <button
        type="button"
        className={button}
        disabled={start.isPending}
        onClick={() => {
          start.mutate();
        }}
      >
        {info.itemsFailed > 0 ? "Try the failed parts again" : "Start embeddings"}
      </button>
      <Failure error={start.error} />
    </>
  );
}

/** "Re-run with Y", with a confirm: the vectors stay until it is done (ADR-0005). */
function RerunAction({ model }: { model: string }) {
  const refresh = useRefresh();
  const [confirming, setConfirming] = useState(false);
  const rerun = useMutation({
    mutationFn: commands.meaningRerun,
    onSettled: () => {
      setConfirming(false);
      void refresh();
    },
  });
  return (
    <>
      {confirming ? (
        <span role="alertdialog" className="flex flex-wrap items-center gap-2 text-sm">
          Re-run with {model}? Meaning search waits until it is done, then uses only the new
          vectors.
          <button
            type="button"
            className={button}
            disabled={rerun.isPending}
            onClick={() => {
              rerun.mutate();
            }}
          >
            Re-run
          </button>
          <button
            type="button"
            className={button}
            onClick={() => {
              setConfirming(false);
            }}
          >
            Keep the vectors
          </button>
        </span>
      ) : (
        <button
          type="button"
          className={button}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Re-run with {model}
        </button>
      )}
      <Failure error={rerun.error} />
    </>
  );
}

function CancelAction({ info }: { info: MeaningStatus }) {
  const refresh = useRefresh();
  const cancel = useMutation({
    mutationFn: commands.meaningCancel,
    onSettled: () => void refresh(),
  });
  return (
    <>
      <button
        type="button"
        className={button}
        disabled={cancel.isPending}
        title={
          info.rerunModel !== null ? "The vectors stay as they are." : "What was made so far stays."
        }
        onClick={() => {
          cancel.mutate();
        }}
      >
        {info.rerunModel !== null ? "Cancel re-run" : "Stop"}
      </button>
      <Failure error={cancel.error} />
    </>
  );
}

/** The panel's "Meaning search" row (M6b): state, progress, destination, what can be done. */
export function MeaningProgressRow({
  info,
  showPause,
}: {
  info: MeaningStatus;
  /** The queue has one Pause; with none showing elsewhere, this row has it. */
  showPause: boolean;
}) {
  const refresh = useRefresh();
  const pause = useMutation({ mutationFn: commands.pause, onSettled: () => void refresh() });
  const resume = useMutation({ mutationFn: commands.resume, onSettled: () => void refresh() });
  if (!meaningRowShown(info)) return null;
  const eta = etaLabel(info.eta);
  const working = info.state === "running" || info.state === "paused";
  const busy = pause.isPending || resume.isPending;
  return (
    <div className="flex w-full min-w-0 flex-wrap items-center gap-2">
      <div className="flex min-w-0 flex-col text-sm" data-meaning-row>
        <span>
          Meaning search · {meaningStateLabel(info.state)}
          {working && ` · ${info.itemsDone.toLocaleString()} of ${plural(info.itemsTotal, "part")}`}
          {eta !== null && ` · ${eta}`}
        </span>
        {info.state === "blocked" && info.reason !== null && (
          <span data-blocked>{info.reason}</span>
        )}
        {info.state === "running" && info.destination !== null && (
          <span className="text-muted">{info.destination}</span>
        )}
        {info.itemsFailed > 0 && (
          <span className="text-muted">{plural(info.itemsFailed, "part")} failed</span>
        )}
      </div>
      {showPause && info.state === "running" && (
        <button
          type="button"
          className={button}
          disabled={busy}
          onClick={() => {
            pause.mutate();
          }}
        >
          {pause.isPending ? "Pausing…" : "Pause"}
        </button>
      )}
      {showPause && info.state === "paused" && (
        <button
          type="button"
          className={button}
          disabled={busy}
          onClick={() => {
            resume.mutate();
          }}
        >
          Resume
        </button>
      )}
      {(working || info.rerunModel !== null) && <CancelAction info={info} />}
      <Failure error={pause.error ?? resume.error} />
    </div>
  );
}

/** Why a query cannot run, and what can be done from here (US-06.2 AC2). */
export function MeaningNotReady({ info }: { info: MeaningStatus }) {
  return (
    <div className="flex flex-col gap-2 px-3 py-2 text-sm" data-meaning-not-ready>
      {info.reason !== null && <p>{info.reason}</p>}
      {info.reason === NO_MODEL && (
        <p className="text-muted">Choose or allow one on the AI models screen.</p>
      )}
      {info.waiting !== null && (
        <p className="text-muted">
          {plural(info.waiting.windows, "passage")} to embed, {aboutLabel(info.waiting.seconds)}.
        </p>
      )}
      <p className="text-muted">Keyword search works in the meantime.</p>
      <div className="flex flex-wrap items-center gap-2">
        {meaningCanStart(info) && info.reason !== NO_MODEL && <StartAction info={info} />}
        {info.rerunWith !== null && <RerunAction model={info.rerunWith} />}
        {info.rerunModel !== null && <CancelAction info={info} />}
      </div>
    </div>
  );
}

/**
 * Meaning search (US-06.2): each query goes to a model, so it runs on Enter, not while typing. The
 * top 20, best first; ↑/↓ move and Enter opens one (NFR-USE-06), or one click (NFR-USE-04).
 */
export function MeaningSearch({
  opened,
  onOpen,
}: {
  /** The result shown in the source pane. */
  opened: SegmentHit | null;
  onOpen: (hit: SearchHit) => void;
}) {
  const status = useMeaning();
  const [text, setText] = useState("");
  const [query, setQuery] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const results = useQuery({
    queryKey: ["meaning", query, status.data?.vectorsModel],
    queryFn: () => commands.meaningSearch(query ?? ""),
    enabled: query !== null && status.data?.state === "ready",
    retry: false,
    gcTime: 0,
    // The index is being made: the same query runs again when it is there.
    refetchInterval: (q) => (q.state.data?.kind === "preparing" ? 1000 : false),
  });
  const info = status.data;
  const ready = info?.state === "ready";
  // Results of vectors no longer searched do not stay on screen under the reason.
  const hits = ready && results.data?.kind === "results" ? results.data.hits : [];
  const open = (i: number) => {
    const row = hits[i];
    if (row === undefined) return;
    setActive(i);
    onOpen(row.hit);
  };
  const isOpened = (h: SearchHit) =>
    opened !== null &&
    h.hit !== null &&
    h.hit.segmentId === opened.segmentId &&
    h.hit.start === opened.start &&
    h.hit.end === opened.end;

  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label="Meaning search">
      <div className="flex flex-col gap-2 border-b border-border px-3 py-2 text-sm">
        <input
          type="search"
          aria-label="Search by meaning"
          placeholder="Describe what you are looking for"
          className="rounded border border-border bg-surface px-2 py-1"
          value={text}
          onChange={(e) => {
            setText(e.target.value);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              setActive(0);
              setQuery(text);
            } else if (e.key === "ArrowDown" && hits.length > 0) {
              e.preventDefault();
              document.getElementById("meaning-results")?.focus();
            }
          }}
        />
        {info !== undefined && ready && info.vectorsModel !== null && (
          <span className="text-xs text-muted">
            Vectors from {info.vectorsModel}
            {info.destination !== null && ` · Queries: ${info.destination}`}
          </span>
        )}
      </div>
      {status.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(status.error)}
        </p>
      )}
      {info !== undefined && !ready && <MeaningNotReady info={info} />}
      {ready && results.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(results.error)}
        </p>
      )}
      {ready && results.isFetching && <p className="px-3 py-2 text-sm text-muted">Searching…</p>}
      {ready && results.data?.kind === "notReady" && (
        <p className="px-3 py-2 text-sm">{results.data.reason}</p>
      )}
      {ready && results.data?.kind === "preparing" && (
        <p className="px-3 py-2 text-sm">Preparing meaning search…</p>
      )}
      {ready && results.data?.kind === "results" && hits.length === 0 && (
        <p className="px-3 py-2 text-sm text-muted">No matches</p>
      )}
      <div
        id="meaning-results"
        role="listbox"
        aria-label="Meaning results"
        tabIndex={hits.length > 0 ? 0 : -1}
        className="min-h-0 flex-1 overflow-auto focus:outline-none"
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const to = active + (e.key === "ArrowDown" ? 1 : -1);
            setActive(Math.max(0, Math.min(hits.length - 1, to)));
          } else if (e.key === "Enter") {
            e.preventDefault();
            open(active);
          }
        }}
      >
        {hits.map((row, i) => (
          <div key={i} role="option" aria-selected={i === active}>
            <SearchResultRow
              hit={row.hit}
              score={row.score}
              active={i === active}
              opened={isOpened(row.hit)}
              onOpen={() => {
                open(i);
              }}
            />
          </div>
        ))}
      </div>
    </section>
  );
}

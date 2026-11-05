import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { Progress } from "../ipc/bindings/Progress";
import { fileName, plural } from "../lib/labels";
import { etaLabel } from "../lib/progress";
import { useMeaning } from "../lib/useMeaning";
import { MeaningProgressRow } from "./Meaning";
import { useAiExtraction } from "../lib/useAiExtraction";
import { AiProgressRow } from "./AiExtraction";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** What the panel shows (US-03.1 AC1–AC2): stage, files, current file, time left. */
export function ProgressSummary({
  progress: p,
  aiShown = false,
}: {
  progress: Progress;
  /** The AI row is there: "Idle" would be wrong, and it says itself why AI work waits. */
  aiShown?: boolean;
}) {
  const eta = etaLabel(p.eta);
  const current = p.currentFiles[0];
  const others = p.currentFiles.length - 1;
  const working = p.stage !== null && (p.ingesting || p.paused);
  return (
    <div role="status" className="flex min-w-0 flex-col text-sm">
      {working && p.stage === "identifiers" ? (
        <span>
          {p.paused ? "Paused" : "Finding identifiers again in stored text"} ·{" "}
          {p.runDone.toLocaleString()} of {plural(p.runTotal, "file")}
        </span>
      ) : working ? (
        <span>
          {p.paused ? "Paused" : "Reading files"} · Stage {p.stage} · {p.runDone.toLocaleString()}{" "}
          of {plural(p.runTotal, "file")}
          {eta !== null && ` · ${eta}`}
        </span>
      ) : p.ingesting ? (
        <span className="text-muted">Looking for new files…</span>
      ) : (
        !aiShown && <span className="text-muted">Idle</span>
      )}
      {!p.paused && current !== undefined && (
        <span className="truncate text-muted" title={p.currentFiles.join("\n")}>
          Now: {fileName(current)}
          {others > 0 && ` and ${String(others)} more`}
        </span>
      )}
      <span className="text-muted">
        {p.done.toLocaleString()} done · {p.failed.toLocaleString()} failed ·{" "}
        {p.queued.toLocaleString()} queued
        {p.notProcessed > 0 && ` · ${p.notProcessed.toLocaleString()} not processed`} ·{" "}
        {plural(p.total, "file")}
      </span>
      {p.blocked !== null && !aiShown && <span data-blocked>AI work waits: {p.blocked}</span>}
    </div>
  );
}

/** The progress panel with Pause/Resume and Cancel (US-03.1, US-03.2, US-03.3). */
export function ProgressPanel({ progress }: { progress: Progress }) {
  const client = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const refresh = () => client.invalidateQueries({ queryKey: ["progress"] });
  const pause = useMutation({ mutationFn: commands.pause, onSettled: refresh });
  const resume = useMutation({ mutationFn: commands.resume, onSettled: refresh });
  const cancel = useMutation({
    mutationFn: commands.cancel,
    onSettled: () => {
      setConfirming(false);
      void refresh();
      void client.invalidateQueries({ queryKey: ["documents"] });
    },
  });
  const busy = pause.isPending || resume.isPending || cancel.isPending;
  const open = progress.stage !== null;
  const error = pause.error ?? resume.error ?? cancel.error;
  const ai = useAiExtraction();
  const aiShown = ai.data !== undefined && ai.data.state !== "idle";
  // The queue has one Pause: the AI row's, when it shows one, stands for both.
  const aiPausable = ai.data?.state === "running" || ai.data?.state === "paused";
  const meaning = useMeaning();

  return (
    <div className="flex min-w-0 flex-1 basis-80 flex-wrap items-center gap-2">
      <ProgressSummary progress={progress} aiShown={aiShown} />
      {open &&
        (progress.paused ? (
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
        ) : (
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
        ))}
      {open &&
        (confirming ? (
          <span className="flex items-center gap-2 text-sm">
            Stop reading? Files not finished will be marked Not processed.
            <button
              type="button"
              className={button}
              disabled={busy}
              onClick={() => {
                cancel.mutate();
              }}
            >
              Stop reading
            </button>
            <button
              type="button"
              className={button}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Keep reading
            </button>
          </span>
        ) : (
          <button
            type="button"
            className={button}
            disabled={busy}
            onClick={() => {
              setConfirming(true);
            }}
          >
            Cancel
          </button>
        ))}
      {error && (
        <span role="alert" className="text-sm">
          {errorMessage(error)}
        </span>
      )}
      {ai.data && <AiProgressRow info={ai.data} showPause={!open} />}
      {meaning.data && <MeaningProgressRow info={meaning.data} showPause={!open && !aiPausable} />}
    </div>
  );
}

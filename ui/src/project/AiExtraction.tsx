import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { AiEstimate } from "../ipc/bindings/AiEstimate";
import type { AiExtractionInfo } from "../ipc/bindings/AiExtractionInfo";
import type { AiScope } from "../ipc/bindings/AiScope";
import { aboutLabel, aiRowShown, aiStateLabel, aiSummaryNotes } from "../lib/aiExtraction";
import { plural } from "../lib/labels";
import { useAiExtraction } from "../lib/useAiExtraction";
import { etaLabel } from "../lib/progress";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** The label on every AI feature, driven by the build's flag (US-10.3). */
export function ExperimentalLabel({ on }: { on: boolean }) {
  if (!on) return null;
  return (
    <span
      className="shrink-0 rounded border border-current px-1 text-xs"
      title="AI results can be wrong or missing. Every one links to the text it came from."
    >
      Experimental
    </span>
  );
}

function useRefresh() {
  const client = useQueryClient();
  return () => client.invalidateQueries({ queryKey: ["progress"] });
}

/** "Re-run with Y", with a confirm: nothing visible changes until it is done (US-10.4). */
export function RerunAction({ model }: { model: string }) {
  const refresh = useRefresh();
  const [confirming, setConfirming] = useState(false);
  const rerun = useMutation({
    mutationFn: commands.aiExtractionRerun,
    onSettled: () => {
      setConfirming(false);
      void refresh();
    },
  });
  return (
    <>
      {confirming ? (
        <span role="alertdialog" className="flex flex-wrap items-center gap-2 text-sm">
          Re-run with {model}? The results stay as they are until it is done, then all change at
          once. Your decisions are kept.
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
            Keep the results
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
      {rerun.error && (
        <span role="alert" className="text-sm">
          {errorMessage(rerun.error)}
        </span>
      )}
    </>
  );
}

/** Cancels the re-run (nothing visible changes), or else the rest of the run. */
function CancelAction({ info }: { info: AiExtractionInfo }) {
  const refresh = useRefresh();
  const cancel = useMutation({
    mutationFn: commands.aiExtractionCancel,
    onSettled: () => void refresh(),
  });
  return (
    <>
      <button
        type="button"
        className={button}
        disabled={cancel.isPending}
        title={
          info.rerunModel !== null
            ? "The results stay as they are."
            : "What was found so far stays."
        }
        onClick={() => {
          cancel.mutate();
        }}
      >
        {info.rerunModel !== null ? "Cancel re-run" : "Stop"}
      </button>
      {cancel.error && (
        <span role="alert" className="text-sm">
          {errorMessage(cancel.error)}
        </span>
      )}
    </>
  );
}

/** The text of the panel's AI row (US-10.2 AC2, US-10.3, INV-PR-09). */
export function AiStatus({ info }: { info: AiExtractionInfo }) {
  const eta = etaLabel(info.eta);
  const notes = aiSummaryNotes(info);
  const working = info.state === "running" || info.state === "paused";
  return (
    <div className="flex min-w-0 flex-col text-sm" data-ai-row>
      <span className="flex flex-wrap items-baseline gap-x-2">
        <span>People and organizations</span>
        <ExperimentalLabel on={info.experimental} />
        <span>
          · {aiStateLabel(info.state)}
          {working && ` · ${info.unitsDone.toLocaleString()} of ${plural(info.unitsTotal, "part")}`}
          {eta !== null && ` · ${eta}`}
        </span>
      </span>
      {info.reason !== null && <span data-blocked>{info.reason}</span>}
      {info.rerunModel !== null && (
        <span className="text-muted">
          Re-running with {info.rerunModel}. The results shown stay until it is done.
        </span>
      )}
      {info.state === "running" && info.destination !== null && (
        <span className="text-muted">{info.destination}</span>
      )}
      {info.state === "waitingForPick" && info.waiting !== null && (
        <span>
          {plural(info.waiting.units, "part")} of text wait, {aboutLabel(info.waiting.seconds)}.
          Choose files in Documents, or find them in all.
        </span>
      )}
      {(info.resultsModel !== null || notes.length > 0) && (
        <span className="text-muted">
          {[
            info.resultsModel !== null &&
              `${plural(info.mentions, "mention")} from ${info.resultsModel}`,
            ...notes,
          ]
            .filter(Boolean)
            .join(" · ")}
        </span>
      )}
    </div>
  );
}

/** The panel's AI row with what can be done from here. */
export function AiProgressRow({
  info,
  showPause,
}: {
  info: AiExtractionInfo;
  /** The panel's own Pause is for while files are read; with none, this row has it. */
  showPause: boolean;
}) {
  const refresh = useRefresh();
  const pause = useMutation({ mutationFn: commands.pause, onSettled: () => void refresh() });
  const resume = useMutation({ mutationFn: commands.resume, onSettled: () => void refresh() });
  if (!aiRowShown(info)) return null;
  const busy = pause.isPending || resume.isPending;
  const error = pause.error ?? resume.error;
  return (
    <div className="flex w-full min-w-0 flex-wrap items-center gap-2">
      <AiStatus info={info} />
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
      {(info.state === "running" || info.state === "paused" || info.rerunModel !== null) && (
        <CancelAction info={info} />
      )}
      {info.state === "waitingForPick" && <FindPeople contentIds={[]} />}
      {info.state !== "applying" && info.rerunWith !== null && (
        <RerunAction model={info.rerunWith} />
      )}
      {error && (
        <span role="alert" className="text-sm">
          {errorMessage(error)}
        </span>
      )}
    </div>
  );
}

/** The confirm before a run: how much it is and about how long (gap A3). */
export function FindConfirm({
  estimate,
  files,
  running,
  onRun,
  onClose,
}: {
  estimate: AiEstimate;
  /** The files picked; none means all. */
  files: number;
  running: boolean;
  onRun: () => void;
  onClose: () => void;
}) {
  const where = files > 0 ? plural(files, "file") : "all files";
  if (estimate.units === 0) {
    return (
      <span role="status" className="flex items-center gap-2 text-sm">
        Nothing new to read in {where}.
        <button type="button" className={button} onClick={onClose}>
          Close
        </button>
      </span>
    );
  }
  return (
    <span role="alertdialog" className="flex flex-wrap items-center gap-2 text-sm">
      Find people and organizations in {where}? {plural(estimate.units, "part")} of text,{" "}
      {aboutLabel(estimate.seconds)}.
      <button type="button" className={button} disabled={running} onClick={onRun}>
        Find them
      </button>
      <button type="button" className={button} onClick={onClose}>
        Not now
      </button>
    </span>
  );
}

/** "Find people and organizations" for the picked files, or all with the estimate (gap A3). */
export function FindPeople({ contentIds }: { contentIds: number[] }) {
  const refresh = useRefresh();
  const status = useAiExtraction();
  const scope: AiScope = contentIds.length > 0 ? { kind: "contents", contentIds } : { kind: "all" };
  const [estimate, setEstimate] = useState<AiEstimate | null>(null);
  const ask = useMutation({
    mutationFn: () => commands.aiExtractionEstimate(scope),
    onSuccess: setEstimate,
  });
  const run = useMutation({
    mutationFn: () => commands.aiExtractionRun(scope),
    onSuccess: () => {
      setEstimate(null);
      void refresh();
    },
  });
  const error = ask.error ?? run.error;
  return (
    <>
      {estimate === null ? (
        <button
          type="button"
          className={button}
          disabled={ask.isPending}
          onClick={() => {
            ask.mutate();
          }}
        >
          Find people and organizations
          {contentIds.length > 0 && ` (${contentIds.length.toLocaleString()})`}
        </button>
      ) : (
        <FindConfirm
          estimate={estimate}
          files={contentIds.length}
          running={run.isPending}
          onRun={() => {
            run.mutate();
          }}
          onClose={() => {
            setEstimate(null);
          }}
        />
      )}
      <ExperimentalLabel on={status.data?.experimental ?? false} />
      {error && (
        <span role="alert" className="text-sm">
          {errorMessage(error)}
        </span>
      )}
    </>
  );
}

/** The Settings section's content (US-10.4 AC1). */
export function AiSettingsBody({ info }: { info: AiExtractionInfo }) {
  return (
    <div className="flex max-w-2xl flex-col gap-2">
      <h3 className="mt-6 flex items-baseline gap-2 font-semibold">
        AI extraction
        <ExperimentalLabel on={info.experimental} />
      </h3>
      <p className="text-sm text-muted">
        People and organizations a model found in this project&apos;s text. Every one links to the
        text it came from; a name the text does not hold is left out.
      </p>
      <p className="text-sm">
        {info.resultsModel !== null ? (
          <>
            The results come from <span className="font-semibold">{info.resultsModel}</span>:{" "}
            {plural(info.mentions, "mention")}.
          </>
        ) : (
          "No results yet."
        )}
      </p>
      {info.rerunModel !== null && (
        <p className="flex flex-wrap items-center gap-2 text-sm">
          Re-running with <span className="font-semibold">{info.rerunModel}</span>:{" "}
          {info.unitsDone.toLocaleString()} of {plural(info.unitsTotal, "part")}. The results stay
          until it is done.
          <CancelAction info={info} />
        </p>
      )}
      {info.state === "applying" && <p className="text-sm">Applying new results…</p>}
      {info.state !== "applying" && info.rerunWith !== null && (
        <div className="flex flex-wrap items-center gap-2">
          <RerunAction model={info.rerunWith} />
        </div>
      )}
    </div>
  );
}

/** The AI extraction section of Settings. */
export function AiExtractionSettings() {
  const status = useAiExtraction();
  if (status.error) {
    return (
      <p role="alert" className="text-sm">
        {errorMessage(status.error)}
      </p>
    );
  }
  return status.data ? <AiSettingsBody info={status.data} /> : null;
}

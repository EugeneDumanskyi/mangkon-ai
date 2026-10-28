import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ConnectionItem } from "../ipc/bindings/ConnectionItem";
import type { Progress } from "../ipc/bindings/Progress";
import type { ProjectInfo } from "../ipc/bindings/ProjectInfo";
import { plural } from "../lib/labels";
import { ConnectionsList } from "./ConnectionsList";
import { OccurrencesPanel } from "./OccurrencesPanel";
import { SourcePane } from "./SourcePane";

/** Counts only in M2d (US-03.1 AC1); stage, ETA and current file come in M3. */
function ProgressLine({ progress }: { progress: Progress }) {
  return (
    <span role="status" className="text-sm text-muted">
      {progress.ingesting ? "Reading files: " : ""}
      {progress.done.toLocaleString()} done · {progress.failed.toLocaleString()} failed ·{" "}
      {progress.queued.toLocaleString()} queued · {plural(progress.total, "file")}
    </span>
  );
}

export function ProjectScreen({ project, uiTiming }: { project: ProjectInfo; uiTiming: boolean }) {
  const client = useQueryClient();
  const [selected, setSelected] = useState<ConnectionItem | null>(null);
  const [mention, setMention] = useState<number | null>(null);
  const progress = useQuery({
    queryKey: ["progress"],
    queryFn: commands.progress,
    // Polled while files are read (US-03.1 AC1); events cover the rest.
    refetchInterval: (q) => (q.state.data?.ingesting ? 500 : false),
  });
  const add = useMutation({
    mutationFn: async () => {
      const path = await commands.pickFolder("Choose a folder to add");
      if (path !== null) await commands.addSource(path);
    },
    onSettled: () => client.invalidateQueries({ queryKey: ["progress"] }),
  });
  const close = useMutation({
    mutationFn: commands.closeProject,
    onSuccess: () => {
      client.setQueryData(["project"], null);
      client.removeQueries({ queryKey: ["connections"] });
    },
  });
  const error = add.error ?? close.error;
  const empty = project.sources.length === 0 && (progress.data?.total ?? 0) === 0;

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2">
        <span className="min-w-0 flex-1 truncate font-mono text-sm" title={project.path}>
          {project.path}
        </span>
        {progress.data && <ProgressLine progress={progress.data} />}
        <button
          type="button"
          className="rounded bg-accent px-3 py-1 text-sm text-accent-text disabled:opacity-50"
          disabled={add.isPending}
          onClick={() => {
            add.mutate();
          }}
        >
          Add folder…
        </button>
        <button
          type="button"
          className="rounded border border-border px-3 py-1 text-sm"
          onClick={() => {
            close.mutate();
          }}
        >
          Close
        </button>
        {error && (
          <p role="alert" className="w-full text-sm">
            {errorMessage(error)}
          </p>
        )}
      </div>
      {empty ? (
        <div className="m-4 flex flex-1 flex-col items-center justify-center gap-2 rounded border-2 border-dashed border-border text-center">
          <p className="text-lg">Drop files or folders here</p>
          <p className="text-muted">or use Add folder…</p>
        </div>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(16rem,1fr)_minmax(14rem,1fr)_minmax(20rem,2fr)]">
          <ConnectionsList
            selectedKey={selected?.key ?? null}
            onSelect={(c) => {
              setSelected(c);
              setMention(null);
            }}
            uiTiming={uiTiming}
          />
          <OccurrencesPanel connection={selected} selected={mention} onSelect={setMention} />
          <SourcePane mentionId={mention} />
        </div>
      )}
    </div>
  );
}

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "./ipc/commands";
import { connectCoreEvents, onCoreEvent } from "./lib/coreEvents";
import { ProjectScreen } from "./project/ProjectScreen";
import { runQueueWalk } from "./project/queueWalk";
import { StartScreen } from "./project/StartScreen";
import { S2dScreen } from "./spike/S2dScreen";

export function App() {
  const client = useQueryClient();
  const info = useQuery({ queryKey: ["app_info"], queryFn: commands.appInfo });
  const project = useQuery({ queryKey: ["project"], queryFn: commands.currentProject });
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    connectCoreEvents(client).catch((e: unknown) => {
      setNotice(`Updates from the core are off: ${errorMessage(e)}`);
    });
    return onCoreEvent((event) => {
      if (event.kind === "error") setNotice(event.message);
      if (event.kind === "ingestFinished" && event.error !== null) setNotice(event.error);
    });
  }, [client]);

  // Scripted runs only (`MK_QUEUE_WALK`): the M3c walk through the project and queue controls.
  const walked = useRef(false);
  const walk = info.data?.queueWalk ?? null;
  useEffect(() => {
    if (walk === null || !project.isSuccess || walked.current) return;
    walked.current = true;
    void runQueueWalk(walk, client);
  }, [walk, project.isSuccess, client]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-baseline gap-3 border-b border-border bg-surface-alt px-4 py-2">
        <h1 className="text-base font-semibold">Mangkon AI</h1>
        {info.data && (
          <span className="text-sm text-muted">
            {info.data.version} · {info.data.ocrEngine}
          </span>
        )}
        {info.error && <span className="text-sm text-muted">Core not reachable</span>}
      </header>
      {notice !== null && (
        <div
          role="alert"
          className="flex items-center gap-3 border-b border-border bg-surface-alt px-4 py-2 text-sm"
        >
          <span className="flex-1">{notice}</span>
          <button
            type="button"
            className="rounded border border-border px-2 py-0.5"
            onClick={() => {
              setNotice(null);
            }}
          >
            Dismiss
          </button>
        </div>
      )}
      <main className="min-h-0 flex-1">
        {info.data?.spikeS2d ? (
          <S2dScreen autorun={info.data.spikeS2d.autorun} />
        ) : project.data ? (
          <ProjectScreen
            project={project.data}
            uiTiming={info.data?.uiTiming ?? false}
            viewSweep={info.data?.viewSweep ?? false}
          />
        ) : (
          project.isSuccess && <StartScreen />
        )}
      </main>
    </div>
  );
}

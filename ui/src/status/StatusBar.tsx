import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { AiActivityInfo } from "../ipc/bindings/AiActivityInfo";
import type { AiReadinessInfo } from "../ipc/bindings/AiReadinessInfo";
import type { RuntimeModel } from "../ipc/bindings/RuntimeModel";
import { firstBlocked, openProjectSettings, roleName, runtimeLine } from "../lib/ai";
import { onCoreEvent } from "../lib/coreEvents";

const link = "underline disabled:opacity-50";

/**
 * The status bar (US-09.8 AC1, US-09.7 AC3, NFR-SEC-05): where data goes while an AI call is on
 * its way, else why the project's AI waits, and each local model's state with Retry.
 */
export function StatusBarView({
  activity,
  readiness,
  models,
  retrying,
  error,
  onRetry,
  onSettings,
}: {
  activity: AiActivityInfo | null;
  readiness: AiReadinessInfo[];
  models: RuntimeModel[];
  retrying: boolean;
  error: string | null;
  onRetry: () => void;
  onSettings: () => void;
}) {
  const blocked = firstBlocked(readiness);
  const lines = models.map((m) => [m, runtimeLine(m)] as const).filter(([, l]) => l !== null);
  const failed = models.some((m) => m.state === "failed");
  return (
    <footer
      aria-label="Status"
      className="flex min-h-7 flex-wrap items-center gap-x-4 gap-y-0.5 border-t border-border bg-surface-alt px-4 py-1 text-sm"
    >
      {activity !== null ? (
        <span role="status" data-activity={activity.class} className="font-semibold">
          {activity.label}
        </span>
      ) : (
        blocked !== null && (
          <span data-blocked={blocked.code ?? ""}>
            {roleName(blocked.role)}: {blocked.reason}
            {(blocked.code === "no_model" || blocked.code === "needs_reconfirmation") && (
              <>
                {" · "}
                <button type="button" className={link} onClick={onSettings}>
                  Settings
                </button>
              </>
            )}
          </span>
        )
      )}
      {lines.map(([m, l]) => (
        <span key={m.role} className="text-muted" data-runtime={m.state}>
          {l}
        </span>
      ))}
      {failed && (
        <button type="button" className={link} disabled={retrying} onClick={onRetry}>
          {retrying ? "Retrying…" : "Retry"}
        </button>
      )}
      {error !== null && <span role="alert">{error}</span>}
    </footer>
  );
}

/** The status bar with its queries; the activity comes from `aiActivity` events. */
export function StatusBar({ projectOpen }: { projectOpen: boolean }) {
  const client = useQueryClient();
  const [activity, setActivity] = useState<AiActivityInfo | null>(null);
  useEffect(() => {
    void commands.aiActivity().then(setActivity, () => undefined);
    return onCoreEvent((event) => {
      if (event.kind === "aiActivity") setActivity(event.activity);
    });
  }, []);
  const readiness = useQuery({
    queryKey: ["ai_readiness"],
    queryFn: commands.aiReadiness,
    enabled: projectOpen,
  });
  const runtime = useQuery({ queryKey: ["runtime"], queryFn: () => commands.runtimeStatus(false) });
  const retry = useMutation({
    mutationFn: commands.runtimeRetry,
    onSuccess: (info) => {
      client.setQueryData(["runtime"], info);
      void client.invalidateQueries({ queryKey: ["ai_readiness"] });
    },
  });
  return (
    <StatusBarView
      activity={activity}
      readiness={projectOpen ? (readiness.data ?? []) : []}
      models={runtime.data?.models ?? []}
      retrying={retry.isPending}
      error={retry.error ? errorMessage(retry.error) : null}
      onRetry={() => {
        retry.mutate();
      }}
      onSettings={openProjectSettings}
    />
  );
}

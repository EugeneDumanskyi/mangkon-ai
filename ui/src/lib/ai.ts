import type { AiReadinessInfo } from "../ipc/bindings/AiReadinessInfo";
import type { ModelRole } from "../ipc/bindings/ModelRole";
import type { RuntimeModel } from "../ipc/bindings/RuntimeModel";
import type { RuntimeState } from "../ipc/bindings/RuntimeState";

/** US-09.7 AC3, as the core words it. */
export const NO_MODEL_ALLOWED = "No model allowed for this project";

/** What a role is for, in the words of the AI models screen. */
export function roleName(role: ModelRole): string {
  return role === "extraction" ? "People and organizations" : "Meaning search";
}

/** A local model's state (domain §7.8) in plain words. */
export function runtimeStateLabel(state: RuntimeState): string {
  switch (state) {
    case "stopped":
      return "Stopped";
    case "starting":
      return "Starting…";
    case "ready":
      return "Ready";
    case "busy":
      return "Working";
    case "stopping":
      return "Stopping…";
    case "crashed":
      return "Stopped unexpectedly, restarting…";
    case "failed":
      return "Failed";
  }
}

/** The status bar line for a local model; `null` while it is stopped. */
export function runtimeLine(m: RuntimeModel): string | null {
  if (m.state === "stopped") return null;
  const base = `${roleName(m.role)}: ${runtimeStateLabel(m.state)}`;
  if (m.state === "failed" && m.failure !== null) return `${base} · ${m.failure.message}`;
  if ((m.state === "ready" || m.state === "busy") && m.port !== null) {
    return `${base} · 127.0.0.1:${String(m.port)}`;
  }
  return base;
}

/** The first role that cannot run, for the status bar. */
export function firstBlocked(readiness: AiReadinessInfo[]): AiReadinessInfo | null {
  return readiness.find((r) => !r.ready) ?? null;
}

type Listener = () => void;
const settingsListeners = new Set<Listener>();

/** Asks the open project to show its Settings tab (the status bar's "Settings" link). */
export function openProjectSettings(): void {
  for (const l of settingsListeners) l();
}

/** Calls `listener` on `openProjectSettings()`; returns the unsubscribe function. */
export function onOpenProjectSettings(listener: Listener): () => void {
  settingsListeners.add(listener);
  return () => {
    settingsListeners.delete(listener);
  };
}

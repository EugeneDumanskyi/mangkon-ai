// Pure labels of meaning search: the Meaning tab and the progress panel's row (US-06.2).
import type { MeaningState } from "../ipc/bindings/MeaningState";
import type { MeaningStatus } from "../ipc/bindings/MeaningStatus";

/** A similarity score, as the row shows it: 0.871 is "0.87". */
export function scoreLabel(score: number): string {
  return score.toFixed(2);
}

/** What the progress panel's row calls each state. */
export function meaningStateLabel(state: MeaningState): string {
  switch (state) {
    case "idle":
      return "Not started";
    case "waitingForStart":
      return "Waiting for you to start it";
    case "running":
      return "Embedding";
    case "paused":
      return "Paused";
    case "blocked":
      return "Blocked";
    case "modelChanged":
      return "Model changed";
    case "preparing":
      return "Preparing…";
    case "ready":
      return "Ready";
  }
}

/** Whether the row has anything to say: not before a run, and not when all is well and quiet. */
export function meaningRowShown(info: MeaningStatus): boolean {
  if (info.state === "idle") return false;
  return info.state !== "ready" || info.itemsFailed > 0;
}

/** The tab offers a start: nothing has run, it is too big to start by itself, or parts failed. */
export function meaningCanStart(info: MeaningStatus): boolean {
  return info.state === "idle" || info.state === "waitingForStart" || info.itemsFailed > 0;
}

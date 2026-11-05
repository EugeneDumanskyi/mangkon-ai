// Pure labels of AI extraction: the progress panel's row, Settings and the Documents action
// (US-10.2 AC2, US-10.4, gap A3).
import type { AiExtractionInfo } from "../ipc/bindings/AiExtractionInfo";
import type { AiLaneState } from "../ipc/bindings/AiLaneState";
import type { Occurrence } from "../ipc/bindings/Occurrence";
import { plural } from "./labels";

/** What the row calls each state. */
export function aiStateLabel(state: AiLaneState): string {
  switch (state) {
    case "idle":
      return "Not started";
    case "waitingForPick":
      return "Waiting for your choice";
    case "running":
      return "Finding them";
    case "paused":
      return "Paused";
    case "blocked":
      return "Blocked";
    case "applying":
      return "Applying new results…";
    case "done":
      return "Done";
  }
}

/** "less than a minute", "about 20 min", "about 2 h 5 min". */
export function aboutLabel(seconds: number): string {
  if (seconds < 60) return "less than a minute";
  if (seconds < 3600) return `about ${String(Math.round(seconds / 60))} min`;
  const h = Math.floor(seconds / 3600);
  const m = Math.round((seconds % 3600) / 60);
  return m === 0 ? `about ${String(h)} h` : `about ${String(h)} h ${String(m)} min`;
}

/**
 * The job summary (US-10.2 AC2, US-10.1 AC2): what the checks left out. Only what happened, so a
 * clean run has no noise.
 */
export function aiSummaryNotes(info: AiExtractionInfo): string[] {
  const notes: string[] = [];
  if (info.discarded > 0) {
    notes.push(`${plural(info.discarded, "mention")} discarded (no matching text)`);
  }
  if (info.invalidResponses > 0) {
    notes.push(`${plural(info.invalidResponses, "response")} could not be read`);
  }
  if (info.unitsFailed > 0) {
    notes.push(`${plural(info.unitsFailed, "part")} failed`);
  }
  return notes;
}

/** Whether the row has anything to say. */
export function aiRowShown(info: AiExtractionInfo): boolean {
  return info.state !== "idle";
}

/** A model's confidence, as a whole percent: 0.87 is "87%". */
export function confidenceLabel(confidence: number): string {
  return `${String(Math.round(confidence * 100))}%`;
}

/** Which model an AI mention came from and how sure it was (US-10.1 AC1, US-10.3, US-10.4 AC1). */
export function aiMentionNote(o: Occurrence, experimental: boolean): string {
  if (o.aiModel === null) return "";
  const label = experimental ? " · Experimental" : "";
  return ` · ${confidenceLabel(o.confidence)} · ${o.aiModel}${label}`;
}

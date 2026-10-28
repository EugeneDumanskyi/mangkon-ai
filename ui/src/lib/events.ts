import type { UiEvent } from "../ipc/bindings/UiEvent";

/** Query keys a core event makes stale. The UI re-queries; events carry no rows (US-07.5). */
export function staleKeys(event: UiEvent): string[][] {
  switch (event.kind) {
    case "connectionsChanged":
      return [["connections"], ["occurrences"], ["source"], ["progress"]];
    case "projectChanged":
      return [["project"], ["progress"], ["connections"], ["occurrences"], ["source"]];
    case "ingestStarted":
      return [["progress"]];
    case "ingestFinished":
      return [["progress"], ["connections"]];
    case "error":
      return [];
  }
}

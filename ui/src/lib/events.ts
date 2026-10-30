import type { UiEvent } from "../ipc/bindings/UiEvent";

/** Query keys a core event makes stale. The UI re-queries; events carry no rows (US-07.5). */
export function staleKeys(event: UiEvent): string[][] {
  switch (event.kind) {
    case "connectionsChanged":
      return [["connections"], ["occurrences"], ["source"], ["progress"], ["documents"]];
    case "projectChanged":
      return [
        ["project"],
        ["progress"],
        ["connections"],
        ["occurrences"],
        ["source"],
        ["documents"],
        ["roots"],
        ["recent"],
        ["search"],
      ];
    case "ingestStarted":
    case "progressChanged":
      return [["progress"]];
    // Search results change only when a run ends, so a list being read does not jump.
    case "ingestFinished":
      return [["progress"], ["connections"], ["documents"], ["search"]];
    case "error":
      return [];
  }
}

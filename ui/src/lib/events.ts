import type { UiEvent } from "../ipc/bindings/UiEvent";

/** Query keys a core event makes stale. The UI re-queries; events carry no rows (US-07.5). */
export function staleKeys(event: UiEvent): string[][] {
  switch (event.kind) {
    case "connectionsChanged":
      return [["connections"], ["occurrences"], ["source"], ["progress"], ["documents"]];
    case "projectChanged":
      return [
        ["project"],
        ["ai_readiness"],
        ["first_run"],
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
      return [["progress"]];
    // AI jobs Blocked or running again also come as progress changes (INV-PR-09).
    case "progressChanged":
      return [["progress"], ["ai_readiness"]];
    // Search results change only when a run ends, so a list being read does not jump.
    case "ingestFinished":
      return [["progress"], ["connections"], ["documents"], ["search"]];
    case "modelsChanged":
      return [["hardware"], ["model_catalog"], ["ai_choice"], ["ai_readiness"]];
    case "runtimeChanged":
      return [["runtime"], ["ai_readiness"]];
    // The status bar takes the activity from the event itself.
    case "aiActivity":
    case "error":
      return [];
  }
}

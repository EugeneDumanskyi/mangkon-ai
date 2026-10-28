import type { QueryClient } from "@tanstack/react-query";
import { commands } from "../ipc/commands";
import type { UiEvent } from "../ipc/bindings/UiEvent";
import { staleKeys } from "./events";

type Listener = (event: UiEvent, receivedAt: number) => void;
const listeners = new Set<Listener>();

/** Calls `listener` for every core event; returns the unsubscribe function. */
export function onCoreEvent(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Subscribes to the core once: each event invalidates its queries, then goes to listeners. */
export function connectCoreEvents(client: QueryClient): Promise<unknown> {
  return commands.subscribe((event) => {
    const receivedAt = performance.now();
    for (const queryKey of staleKeys(event)) void client.invalidateQueries({ queryKey });
    for (const l of listeners) l(event, receivedAt);
  });
}

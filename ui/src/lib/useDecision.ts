import { useMutation, useQueryClient } from "@tanstack/react-query";

/** What a decision can change on screen (US-11.5). */
const AFTER_DECISION = [
  ["review"],
  ["entity"],
  ["decisions"],
  ["connections"],
  ["occurrences"],
  ["source"],
  ["progress"],
];

/**
 * A user's decision: its error stays on screen, and everything it can change is read again. Not
 * every decision changes a connection, so no event can be counted on for this.
 */
export function useDecision() {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (decide: () => Promise<unknown>) => decide(),
    onSettled: () =>
      Promise.all(AFTER_DECISION.map((queryKey) => client.invalidateQueries({ queryKey }))),
  });
}

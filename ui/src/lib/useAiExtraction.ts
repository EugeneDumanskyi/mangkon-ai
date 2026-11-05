import { useQuery } from "@tanstack/react-query";
import { commands } from "../ipc/commands";

/** What AI extraction is doing, kept fresh by the progress events. */
export function useAiExtraction() {
  return useQuery({
    queryKey: ["progress", "ai"],
    queryFn: commands.aiExtractionStatus,
    refetchOnMount: "always",
  });
}

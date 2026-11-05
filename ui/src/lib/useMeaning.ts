import { useQuery } from "@tanstack/react-query";
import { commands } from "../ipc/commands";

/** What meaning search can do now, kept fresh by the progress events. */
export function useMeaning() {
  return useQuery({
    queryKey: ["progress", "meaning"],
    queryFn: commands.meaningStatus,
    refetchOnMount: "always",
  });
}

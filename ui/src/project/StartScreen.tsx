import { useMutation, useQueryClient } from "@tanstack/react-query";
import { commands, errorMessage } from "../ipc/commands";

/** No project open (gap A7: one at a time). A project is a folder; an empty one becomes new. */
export function StartScreen() {
  const client = useQueryClient();
  const open = useMutation({
    mutationFn: async (title: string) => {
      const path = await commands.pickFolder(title);
      return path === null ? null : commands.openProject(path);
    },
    onSuccess: (info) => {
      if (info) client.setQueryData(["project"], info);
    },
  });

  const button = "rounded px-4 py-2 disabled:opacity-50";
  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 p-4 text-center">
      <h2 className="text-lg font-semibold">No project open</h2>
      <p className="max-w-md text-muted">
        A project is a folder where Mangkon keeps what it finds. Pick an empty folder for a new
        project, or a folder that already holds one.
      </p>
      <div className="flex gap-3">
        <button
          type="button"
          className={`${button} bg-accent text-accent-text`}
          disabled={open.isPending}
          onClick={() => {
            open.mutate("Choose an empty folder for the new project");
          }}
        >
          New project…
        </button>
        <button
          type="button"
          className={`${button} border border-border`}
          disabled={open.isPending}
          onClick={() => {
            open.mutate("Choose a project folder");
          }}
        >
          Open project…
        </button>
      </div>
      {open.error && (
        <p role="alert" className="max-w-md">
          {errorMessage(open.error)}
        </p>
      )}
    </div>
  );
}

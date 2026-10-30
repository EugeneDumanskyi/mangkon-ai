import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorCode, errorMessage } from "../ipc/commands";
import { createErrorField } from "../lib/forms";
import type { ProjectInfo } from "../ipc/bindings/ProjectInfo";
import type { RecentProject } from "../ipc/bindings/RecentProject";

const button = "rounded px-3 py-1 text-sm disabled:opacity-50";
const plain = `${button} border border-border`;
const primary = `${button} bg-accent text-accent-text`;

/** Recent projects, newest first (US-01.2 AC1); a missing one is marked. */
export function RecentList({
  projects,
  disabled,
  onOpen,
}: {
  projects: RecentProject[];
  disabled: boolean;
  onOpen: (p: RecentProject) => void;
}) {
  if (projects.length === 0) {
    return <p className="text-sm text-muted">No recent projects.</p>;
  }
  return (
    <ul aria-label="Recent projects" className="flex flex-col">
      {projects.map((p) => (
        <li key={p.id}>
          <button
            type="button"
            disabled={disabled}
            className="flex w-full flex-col items-start rounded px-3 py-2 text-left hover:bg-surface-alt disabled:opacity-50"
            onClick={() => {
              onOpen(p);
            }}
          >
            <span className="font-medium">
              {p.name}
              {!p.present && <span className="ml-2 text-sm text-muted">(not found)</span>}
            </span>
            <span className="w-full truncate font-mono text-xs text-muted" title={p.path}>
              {p.path}
            </span>
            <span className="text-xs text-muted">
              Last opened {new Date(p.lastOpenedAt).toLocaleString()}
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}

/** A recent project whose folder is gone (US-01.2 AC2). */
export function MissingProject({
  project,
  busy,
  error,
  onLocate,
  onForget,
  onClose,
}: {
  project: RecentProject;
  busy: boolean;
  error: string | null;
  onLocate: () => void;
  onForget: () => void;
  onClose: () => void;
}) {
  return (
    <div role="alertdialog" className="flex flex-col gap-2 rounded border border-border p-3">
      <p>
        Project not found at <span className="font-mono">{project.path}</span>
      </p>
      <div className="flex gap-2">
        <button type="button" className={primary} disabled={busy} onClick={onLocate}>
          Locate…
        </button>
        <button type="button" className={plain} disabled={busy} onClick={onForget}>
          Remove from list
        </button>
        <button type="button" className={plain} onClick={onClose}>
          Close
        </button>
      </div>
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
    </div>
  );
}

/** The New project form (US-01.1): a name and a location; the folder is `<location>/<name>`. */
export function NewProjectForm({
  location,
  pending,
  error,
  onPickLocation,
  onCreate,
  onCancel,
}: {
  location: string | null;
  pending: boolean;
  error: unknown;
  onPickLocation: () => void;
  onCreate: (name: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const field = error ? createErrorField(errorCode(error)) : null;
  const message = error ? errorMessage(error) : null;
  return (
    <form
      aria-label="New project"
      className="flex w-full max-w-md flex-col gap-3 rounded border border-border p-4 text-left"
      onSubmit={(e) => {
        e.preventDefault();
        onCreate(name);
      }}
    >
      <h3 className="font-semibold">New project</h3>
      <label className="flex flex-col gap-1 text-sm">
        Name
        <input
          className="rounded border border-border bg-surface px-2 py-1"
          value={name}
          maxLength={100}
          aria-invalid={field === "name"}
          onChange={(e) => {
            setName(e.target.value);
          }}
        />
        {field === "name" && <span role="alert">{message}</span>}
      </label>
      <div className="flex flex-col gap-1 text-sm">
        Location
        <div className="flex items-center gap-2">
          <span className="min-w-0 flex-1 truncate font-mono text-xs" title={location ?? ""}>
            {location ?? "No folder chosen"}
          </span>
          <button type="button" className={plain} onClick={onPickLocation}>
            Choose…
          </button>
        </div>
        {field === "location" && <span role="alert">{message}</span>}
      </div>
      {message !== null && field === null && (
        <p role="alert" className="text-sm">
          {message}
        </p>
      )}
      <div className="flex gap-2">
        <button
          type="submit"
          className={primary}
          disabled={pending || name.trim() === "" || location === null}
        >
          Create
        </button>
        <button type="button" className={plain} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** No project open (gap A7: one at a time): recent projects, New project and Open…. */
export function StartScreen() {
  const client = useQueryClient();
  const recent = useQuery({ queryKey: ["recent"], queryFn: commands.recentProjects });
  const [creating, setCreating] = useState(false);
  const [location, setLocation] = useState<string | null>(null);
  const [missing, setMissing] = useState<RecentProject | null>(null);
  const opened = (info: ProjectInfo | null) => {
    if (info) client.setQueryData(["project"], info);
  };
  const refreshRecent = () => client.invalidateQueries({ queryKey: ["recent"] });

  const open = useMutation({
    mutationFn: async (p: RecentProject | null) => {
      if (p !== null) return commands.openRecent(p.id);
      const path = await commands.pickFolder("Choose a project folder");
      return path === null ? null : commands.openProject(path);
    },
    onSuccess: opened,
    onError: (e, p) => {
      if (p !== null && errorCode(e) === "not_found") setMissing(p);
      void refreshRecent();
    },
  });
  const create = useMutation({
    mutationFn: (name: string) => commands.createProject(name, location ?? ""),
    onSuccess: opened,
  });
  const locate = useMutation({
    mutationFn: async (p: RecentProject) => {
      const path = await commands.pickFolder(`Locate ${p.name}`);
      if (path === null) return null;
      await commands.locateProject(p.id, path);
      return commands.openRecent(p.id);
    },
    onSuccess: (info) => {
      if (info) setMissing(null);
      opened(info);
      void refreshRecent();
    },
  });
  const forget = useMutation({
    mutationFn: (p: RecentProject) => commands.forgetProject(p.id),
    onSuccess: () => {
      setMissing(null);
      void refreshRecent();
    },
  });
  const busy = open.isPending || create.isPending || locate.isPending || forget.isPending;

  return (
    <div className="flex h-full flex-col items-center gap-6 overflow-auto p-6">
      <div className="flex flex-col items-center gap-2 text-center">
        <h2 className="text-lg font-semibold">No project open</h2>
        <p className="max-w-md text-muted">
          A project keeps what Mangkon finds in your files. Your files stay where they are.
        </p>
      </div>
      <div className="flex gap-3">
        <button
          type="button"
          className={primary}
          disabled={busy}
          onClick={() => {
            create.reset();
            setCreating(true);
          }}
        >
          New project…
        </button>
        <button
          type="button"
          className={plain}
          disabled={busy}
          onClick={() => {
            open.mutate(null);
          }}
        >
          Open…
        </button>
      </div>
      {creating && (
        <NewProjectForm
          location={location}
          pending={create.isPending}
          error={create.error}
          onPickLocation={() => {
            void commands.pickFolder("Choose where to make the project folder").then((p) => {
              if (p !== null) setLocation(p);
            });
          }}
          onCreate={(name) => {
            create.mutate(name);
          }}
          onCancel={() => {
            setCreating(false);
          }}
        />
      )}
      {missing !== null && (
        <MissingProject
          project={missing}
          busy={busy}
          error={
            locate.error
              ? errorMessage(locate.error)
              : forget.error
                ? errorMessage(forget.error)
                : null
          }
          onLocate={() => {
            locate.mutate(missing);
          }}
          onForget={() => {
            forget.mutate(missing);
          }}
          onClose={() => {
            setMissing(null);
          }}
        />
      )}
      {open.error && errorCode(open.error) !== "not_found" && (
        <p role="alert" className="max-w-md text-center">
          {errorMessage(open.error)}
        </p>
      )}
      <section className="w-full max-w-xl">
        <h3 className="mb-2 font-semibold">Recent projects</h3>
        {recent.error && <p role="alert">{errorMessage(recent.error)}</p>}
        {recent.data && (
          <RecentList
            projects={recent.data}
            disabled={busy}
            onOpen={(p) => {
              if (p.present) open.mutate(p);
              else setMissing(p);
            }}
          />
        )}
      </section>
    </div>
  );
}

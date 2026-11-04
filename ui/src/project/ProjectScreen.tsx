import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { ClickSweepInfo } from "../ipc/bindings/ClickSweepInfo";
import type { ProjectInfo } from "../ipc/bindings/ProjectInfo";
import { onOpenProjectSettings } from "../lib/ai";
import { onCoreEvent } from "../lib/coreEvents";
import {
  type History,
  type Place,
  START,
  applyScrolls,
  back,
  go,
  readScrolls,
  restored,
  viewKey,
} from "../lib/history";
import { addSummary, skippedLines } from "../lib/progress";
import { ConnectionsList } from "./ConnectionsList";
import { DocumentsList } from "./DocumentsList";
import { OccurrencesPanel } from "./OccurrencesPanel";
import { ProgressPanel } from "./ProgressPanel";
import { SearchPanel } from "./SearchPanel";
import { SettingsPanel } from "./SettingsPanel";
import { SourcePane } from "./SourcePane";
import { runClickSweep, runViewSweep } from "./viewSweep";

type Tab = "connections" | "search" | "documents" | "settings";
const TABS: [Tab, string][] = [
  ["connections", "Connections"],
  ["search", "Search"],
  ["documents", "Documents"],
  ["settings", "Settings"],
];

interface Summary {
  text: string;
  skipped: string[];
}

/** The add summary after a run that added or skipped files (US-01.3 AC5). */
export function AddSummary({ summary, onDismiss }: { summary: Summary; onDismiss: () => void }) {
  return (
    <div
      role="status"
      className="flex flex-col gap-1 border-b border-border bg-surface-alt px-4 py-2 text-sm"
    >
      <div className="flex items-center gap-3">
        <span className="flex-1">{summary.text}</span>
        <button
          type="button"
          className="rounded border border-border px-2 py-0.5"
          onClick={onDismiss}
        >
          Dismiss
        </button>
      </div>
      {summary.skipped.length > 0 && (
        <details>
          <summary className="cursor-pointer text-muted">Skipped files</summary>
          <ul className="max-h-40 overflow-auto font-mono text-xs">
            {summary.skipped.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

export function ProjectScreen({
  project,
  uiTiming,
  viewSweep = false,
  clickSweep = null,
}: {
  project: ProjectInfo;
  uiTiming: boolean;
  viewSweep?: boolean;
  /** The click sweep (NFR-PERF-14, NFR-QA-12), for scripted runs only. */
  clickSweep?: ClickSweepInfo | null;
}) {
  const client = useQueryClient();
  // Where the connections screen is, and where it was (US-08.2 AC2).
  const [history, setHistory] = useState<History>(START);
  const { connection: selected, mention, document: openedDocument, hit } = history.place;
  const goTo = useCallback((place: Place) => {
    const scroll = readScrolls();
    setHistory((h) => go(h, place, scroll));
  }, []);
  const goBack = useCallback(() => {
    setHistory(back);
  }, []);
  const [tab, setTab] = useState<Tab>("connections");
  // The status bar's "Settings" link (US-09.7 AC3).
  useEffect(
    () =>
      onOpenProjectSettings(() => {
        setTab("settings");
      }),
    [],
  );
  const [summary, setSummary] = useState<Summary | null>(null);
  const progress = useQuery({
    queryKey: ["progress"],
    queryFn: commands.progress,
    // `progressChanged` events drive it while files are read (US-03.1 AC1); a slow poll is the
    // fallback.
    refetchInterval: (q) => (q.state.data?.ingesting ? 2_000 : false),
  });
  useEffect(
    () =>
      onCoreEvent((event) => {
        if (event.kind !== "ingestFinished") return;
        const text = addSummary(event);
        if (text !== null) setSummary({ text, skipped: skippedLines(event) });
      }),
    [],
  );
  const add = useMutation({
    mutationFn: async (what: "folder" | "files") => {
      const paths =
        what === "folder"
          ? [await commands.pickFolder("Choose a folder to add")].filter((p) => p !== null)
          : await commands.pickFiles("Choose files to add");
      for (const path of paths) await commands.addSource(path);
    },
    onSettled: () => client.invalidateQueries({ queryKey: ["progress"] }),
  });
  const rescan = useMutation({
    mutationFn: commands.rescan,
    onSettled: () => client.invalidateQueries({ queryKey: ["progress"] }),
  });
  const close = useMutation({
    mutationFn: commands.closeProject,
    onSuccess: () => {
      client.setQueryData(["project"], null);
      client.removeQueries({ queryKey: ["connections"] });
    },
  });
  const error = add.error ?? rescan.error ?? close.error;

  // Scripted runs only: once every file is read, open every source view (NFR-SEC-09 check).
  const swept = useRef(false);
  const p = progress.data;
  const finished = p !== undefined && !p.ingesting && p.total > 0 && p.queued === 0;
  useEffect(() => {
    if (!viewSweep || !finished || swept.current) return;
    swept.current = true;
    void runViewSweep((c, m) => {
      goTo({ connection: c, mention: m, document: null, hit: null });
    }).then(commands.recordViewSweep);
  }, [viewSweep, finished, goTo]);
  useEffect(() => {
    if (clickSweep === null || !finished || swept.current) return;
    swept.current = true;
    void runClickSweep(clickSweep, (c) => {
      goTo({ connection: c, mention: null, document: null, hit: null });
    }).then(commands.recordViewSweep);
  }, [clickSweep, finished, goTo]);

  // Back restores the panes' scroll once they show the place again: the source pane's view,
  // and the list rows, may still be loading.
  const restore = history.restore;
  const want = viewKey(history.place);
  useEffect(() => {
    if (restore === null) return;
    let frame = 0;
    const end = performance.now() + 5_000;
    const tick = () => {
      const pane = document.querySelector<HTMLElement>('section[aria-label="Source"]');
      const img = pane?.querySelector("img");
      const ready = (want === null || pane?.dataset.view === want) && (!img || img.complete);
      if (ready || performance.now() > end) {
        applyScrolls(restore);
        setHistory(restored);
      } else {
        frame = requestAnimationFrame(tick);
      }
    };
    frame = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(frame);
    };
  }, [restore, want]);
  // Alt+← goes back, as in a browser.
  useEffect(() => {
    if (tab !== "connections" && tab !== "search") return;
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey && e.key === "ArrowLeft") {
        e.preventDefault();
        goBack();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [tab, goBack]);
  const empty = project.sources.length === 0 && (progress.data?.total ?? 0) === 0;

  const button = "rounded border border-border px-3 py-1 text-sm disabled:opacity-50";
  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-3 border-b border-border px-4 py-2">
        <span className="min-w-0 truncate" title={project.path}>
          <span className="font-semibold">{project.name}</span>{" "}
          <span className="font-mono text-xs text-muted">{project.path}</span>
        </span>
        {progress.data && <ProgressPanel progress={progress.data} />}
        <button
          type="button"
          className="rounded bg-accent px-3 py-1 text-sm text-accent-text disabled:opacity-50"
          disabled={add.isPending}
          onClick={() => {
            add.mutate("folder");
          }}
        >
          Add folder…
        </button>
        <button
          type="button"
          className={button}
          disabled={add.isPending}
          onClick={() => {
            add.mutate("files");
          }}
        >
          Add files…
        </button>
        <button
          type="button"
          className={button}
          disabled={rescan.isPending || project.sources.length === 0}
          onClick={() => {
            rescan.mutate();
          }}
        >
          Re-scan
        </button>
        <button
          type="button"
          className={button}
          onClick={() => {
            close.mutate();
          }}
        >
          Close
        </button>
        {error && (
          <p role="alert" className="w-full text-sm">
            {errorMessage(error)}
          </p>
        )}
      </div>
      {summary !== null && (
        <AddSummary
          summary={summary}
          onDismiss={() => {
            setSummary(null);
          }}
        />
      )}
      <div
        role="tablist"
        aria-label="Project views"
        className="flex gap-1 border-b border-border px-4"
      >
        {TABS.map(([id, label]) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className={`border-b-2 px-3 py-1.5 text-sm ${
              tab === id ? "border-accent font-semibold" : "border-transparent text-muted"
            }`}
            onClick={() => {
              setTab(id);
            }}
          >
            {label}
          </button>
        ))}
      </div>
      <div role="tabpanel" className="min-h-0 flex-1">
        {tab === "documents" ? (
          <DocumentsList />
        ) : tab === "search" ? (
          <div className="grid h-full min-h-0 grid-cols-[minmax(20rem,2fr)_minmax(20rem,3fr)]">
            <SearchPanel
              opened={hit}
              onOpen={(h) => {
                goTo({ connection: selected, mention: null, document: null, hit: h.hit });
              }}
            />
            <SourcePane
              mentionId={mention}
              documentId={openedDocument}
              hit={hit}
              empty="Pick a result to see it in its file."
              canBack={history.back.length > 0}
              onBack={goBack}
              onMention={(m) => {
                goTo({ connection: selected, mention: m, document: null, hit: null });
              }}
              onOpenDocument={(d) => {
                goTo({ ...history.place, document: d });
              }}
            />
          </div>
        ) : tab === "settings" ? (
          <SettingsPanel />
        ) : empty ? (
          <div className="m-4 flex h-[calc(100%-2rem)] flex-col items-center justify-center gap-2 rounded border-2 border-dashed border-border text-center">
            <p className="text-lg">Drop files or folders here</p>
            <p className="text-muted">or use Add folder… or Add files…</p>
          </div>
        ) : (
          <div className="grid h-full min-h-0 grid-cols-[minmax(16rem,1fr)_minmax(14rem,1fr)_minmax(20rem,2fr)]">
            <ConnectionsList
              selectedKey={selected?.key ?? null}
              onSelect={(c) => {
                goTo({ connection: c, mention: null, document: null, hit: null });
              }}
              uiTiming={uiTiming}
            />
            <OccurrencesPanel
              connection={selected}
              selected={mention}
              onSelect={(m) => {
                goTo({ connection: selected, mention: m, document: null, hit: null });
              }}
            />
            <SourcePane
              mentionId={mention}
              documentId={openedDocument}
              hit={hit}
              canBack={history.back.length > 0}
              onBack={goBack}
              onMention={(m) => {
                goTo({ connection: selected, mention: m, document: null, hit: null });
              }}
              onOpenDocument={(d) => {
                goTo({ ...history.place, document: d });
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}

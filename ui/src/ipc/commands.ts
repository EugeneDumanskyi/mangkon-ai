// The only module that calls into the core (ADR-0003). Every argument and result type comes from
// the generated bindings, so a change on the Rust side breaks the TypeScript build.
import { Channel, invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "./bindings/AppInfo";
import type { ConnectionCursor } from "./bindings/ConnectionCursor";
import type { ConnectionsPage } from "./bindings/ConnectionsPage";
import type { DocumentCursor } from "./bindings/DocumentCursor";
import type { DocumentFilter } from "./bindings/DocumentFilter";
import type { DocumentsPage } from "./bindings/DocumentsPage";
import type { DocumentType } from "./bindings/DocumentType";
import type { FrameStats } from "./bindings/FrameStats";
import type { IpcError } from "./bindings/IpcError";
import type { Occurrence } from "./bindings/Occurrence";
import type { PageView } from "./bindings/PageView";
import type { Progress } from "./bindings/Progress";
import type { ProjectInfo } from "./bindings/ProjectInfo";
import type { QueueWalk } from "./bindings/QueueWalk";
import type { RecentProject } from "./bindings/RecentProject";
import type { RemoveSummary } from "./bindings/RemoveSummary";
import type { RowPage } from "./bindings/RowPage";
import type { RowsRequest } from "./bindings/RowsRequest";
import type { Settings } from "./bindings/Settings";
import type { SettingsInfo } from "./bindings/SettingsInfo";
import type { SettingsSaved } from "./bindings/SettingsSaved";
import type { SourceRoot } from "./bindings/SourceRoot";
import type { SourceView } from "./bindings/SourceView";
import type { UiEvent } from "./bindings/UiEvent";
import type { UiTiming } from "./bindings/UiTiming";
import type { ViewSweep } from "./bindings/ViewSweep";

/** The plain message of a failed command (NFR-USE-05). */
export function errorMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    return (e as IpcError).message;
  }
  return String(e);
}

/** The stable code of a failed command, such as `name_taken`; `null` for other errors. */
export function errorCode(e: unknown): string | null {
  if (typeof e === "object" && e !== null && "code" in e) {
    return (e as IpcError).code;
  }
  return null;
}

export const commands = {
  appInfo: () => invoke<AppInfo>("app_info"),
  /** The native folder dialog; `null` when cancelled. */
  pickFolder: (title: string) => invoke<string | null>("pick_folder", { title }),
  /** The native file dialog, several files at once; empty when cancelled. */
  pickFiles: (title: string) => invoke<string[]>("pick_files", { title }),
  recentProjects: () => invoke<RecentProject[]>("recent_projects"),
  createProject: (name: string, location: string) =>
    invoke<ProjectInfo>("create_project", { name, location }),
  /** Opens the project in a folder (Open…). */
  openProject: (path: string) => invoke<ProjectInfo>("open_project", { path, id: null }),
  /** Opens a project of the recent list. */
  openRecent: (id: number) => invoke<ProjectInfo>("open_project", { path: null, id }),
  locateProject: (id: number, path: string) =>
    invoke<RecentProject>("locate_project", { id, path }),
  forgetProject: (id: number) => invoke<null>("forget_project", { id }),
  closeProject: () => invoke<null>("close_project"),
  currentProject: () => invoke<ProjectInfo | null>("current_project"),
  addSource: (path: string) => invoke<null>("add_source", { path }),
  rescan: () => invoke<null>("rescan"),
  progress: () => invoke<Progress>("progress"),
  /** Returns once no file is being read (within 5 s). */
  pause: () => invoke<null>("pause"),
  resume: () => invoke<null>("resume"),
  /** Returns how many files are now Not processed. */
  cancel: () => invoke<number>("cancel"),
  documentsPage: (filter: DocumentFilter, cursor: DocumentCursor | null, limit: number) =>
    invoke<DocumentsPage>("documents_page", { filter, cursor, limit }),
  documentsCount: (filter: DocumentFilter) => invoke<number>("documents_count", { filter }),
  documentTypes: () => invoke<DocumentType[]>("document_types"),
  /** Reads these failed or not-processed files again, or all of them for `null`. */
  retry: (contentIds: number[] | null) => invoke<number>("retry", { contentIds }),
  removalImpact: (refIds: number[]) => invoke<number>("removal_impact", { refIds }),
  removeFiles: (refIds: number[]) => invoke<RemoveSummary>("remove_files", { refIds }),
  projectSettings: () => invoke<SettingsInfo>("project_settings"),
  saveSettings: (settings: Settings) => invoke<SettingsSaved>("save_settings", { settings }),
  /** Reads again the files the OCR languages change; returns how many. */
  reprocess: () => invoke<number>("reprocess"),
  sourceRoots: () => invoke<SourceRoot[]>("source_roots"),
  relocateRoot: (root: number, path: string) => invoke<null>("relocate_root", { root, path }),
  removeRoot: (root: number) => invoke<RemoveSummary>("remove_root", { root }),
  connectionCount: () => invoke<number>("connection_count"),
  connectionsPage: (cursor: ConnectionCursor | null, limit: number) =>
    invoke<ConnectionsPage>("connections_page", { cursor, limit }),
  occurrences: (key: string) => invoke<Occurrence[]>("occurrences", { key }),
  sourceContext: (mentionId: number) => invoke<SourceView>("source_context", { mentionId }),
  /** A document from its start: an attachment, or the email it came in. */
  sourceDocument: (documentId: number) => invoke<SourceView>("source_document", { documentId }),
  /** One page of a PDF or image, with the boxes of `mentionId` when it is on that page. */
  pageView: (documentId: number, page: number, mentionId: number | null) =>
    invoke<PageView>("page_view", { documentId, page, mentionId }),
  /** The PNG of a page `pageView` has shown, as raw bytes. */
  pageImage: (documentId: number, page: number) =>
    invoke<ArrayBuffer>("page_image", { documentId, page }),
  /** Core events go to `onEvent` until the page reloads (no `core:event` permission). */
  subscribe: (onEvent: (event: UiEvent) => void) => {
    const channel = new Channel<UiEvent>();
    channel.onmessage = onEvent;
    return invoke<null>("subscribe", { channel });
  },
  /** `MK_UI_TIMING=1` only. Returns the path the timings were written to. */
  recordUiTiming: (timing: UiTiming) => invoke<string>("record_ui_timing", { timing }),
  /** `MK_VIEW_SWEEP=1` only. Saves the sweep and exits the app. */
  recordViewSweep: (sweep: ViewSweep) => invoke<string>("record_view_sweep", { sweep }),
  /** `MK_QUEUE_WALK=1` or `2` only. Saves the walk and exits the app. */
  recordQueueWalk: (walk: QueueWalk) => invoke<string>("record_queue_walk", { walk }),
  /** Spike S2d builds only. */
  syntheticRows: (request: RowsRequest) => invoke<RowPage>("synthetic_rows", { request }),
  /** Spike S2d builds only. Returns the path the stats were written to. */
  recordFrameStats: (stats: FrameStats) => invoke<string>("record_frame_stats", { stats }),
};

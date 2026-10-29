// The only module that calls into the core (ADR-0003). Every argument and result type comes from
// the generated bindings, so a change on the Rust side breaks the TypeScript build.
import { Channel, invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "./bindings/AppInfo";
import type { ConnectionCursor } from "./bindings/ConnectionCursor";
import type { ConnectionsPage } from "./bindings/ConnectionsPage";
import type { FrameStats } from "./bindings/FrameStats";
import type { IpcError } from "./bindings/IpcError";
import type { Occurrence } from "./bindings/Occurrence";
import type { PageView } from "./bindings/PageView";
import type { Progress } from "./bindings/Progress";
import type { ProjectInfo } from "./bindings/ProjectInfo";
import type { RowPage } from "./bindings/RowPage";
import type { RowsRequest } from "./bindings/RowsRequest";
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

export const commands = {
  appInfo: () => invoke<AppInfo>("app_info"),
  /** The native folder dialog; `null` when cancelled. */
  pickFolder: (title: string) => invoke<string | null>("pick_folder", { title }),
  openProject: (path: string) => invoke<ProjectInfo>("open_project", { path }),
  closeProject: () => invoke<null>("close_project"),
  currentProject: () => invoke<ProjectInfo | null>("current_project"),
  addSource: (path: string) => invoke<null>("add_source", { path }),
  progress: () => invoke<Progress>("progress"),
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
  /** Spike S2d builds only. */
  syntheticRows: (request: RowsRequest) => invoke<RowPage>("synthetic_rows", { request }),
  /** Spike S2d builds only. Returns the path the stats were written to. */
  recordFrameStats: (stats: FrameStats) => invoke<string>("record_frame_stats", { stats }),
};

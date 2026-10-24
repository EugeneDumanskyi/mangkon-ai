// The only module that calls into the core (ADR-0003). Every argument and result type comes from
// the generated bindings, so a change on the Rust side breaks the TypeScript build.
import { invoke } from "@tauri-apps/api/core";
import type { AppInfo } from "./bindings/AppInfo";
import type { FrameStats } from "./bindings/FrameStats";
import type { RowPage } from "./bindings/RowPage";
import type { RowsRequest } from "./bindings/RowsRequest";

export const commands = {
  appInfo: () => invoke<AppInfo>("app_info"),
  /** Spike S2d builds only. */
  syntheticRows: (request: RowsRequest) => invoke<RowPage>("synthetic_rows", { request }),
  /** Spike S2d builds only. Returns the path the stats were written to. */
  recordFrameStats: (stats: FrameStats) => invoke<string>("record_frame_stats", { stats }),
};

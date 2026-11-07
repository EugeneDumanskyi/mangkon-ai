// The only module that calls into the core (ADR-0003). Every argument and result type comes from
// the generated bindings, so a change on the Rust side breaks the TypeScript build.
import { Channel, invoke } from "@tauri-apps/api/core";
import type { AiActivityInfo } from "./bindings/AiActivityInfo";
import type { AiChoice } from "./bindings/AiChoice";
import type { AiChoiceInfo } from "./bindings/AiChoiceInfo";
import type { AiEstimate } from "./bindings/AiEstimate";
import type { AiExtractionInfo } from "./bindings/AiExtractionInfo";
import type { AiReadinessInfo } from "./bindings/AiReadinessInfo";
import type { AiScope } from "./bindings/AiScope";
import type { MeaningEstimate } from "./bindings/MeaningEstimate";
import type { MeaningResult } from "./bindings/MeaningResult";
import type { MeaningStatus } from "./bindings/MeaningStatus";
import type { AllowPrompt } from "./bindings/AllowPrompt";
import type { AppInfo } from "./bindings/AppInfo";
import type { CatalogModel } from "./bindings/CatalogModel";
import type { ColumnAssignment } from "./bindings/ColumnAssignment";
import type { ConnectionCursor } from "./bindings/ConnectionCursor";
import type { ConnectionFilter } from "./bindings/ConnectionFilter";
import type { ConnectionsPage } from "./bindings/ConnectionsPage";
import type { ConnectionsSummary } from "./bindings/ConnectionsSummary";
import type { CustomPatternItem } from "./bindings/CustomPatternItem";
import type { Destination } from "./bindings/Destination";
import type { DestinationClass } from "./bindings/DestinationClass";
import type { DocumentCursor } from "./bindings/DocumentCursor";
import type { DocumentFilter } from "./bindings/DocumentFilter";
import type { DocumentsPage } from "./bindings/DocumentsPage";
import type { DecisionsPage } from "./bindings/DecisionsPage";
import type { DocumentType } from "./bindings/DocumentType";
import type { EgressRecord } from "./bindings/EgressRecord";
import type { EntityDetail } from "./bindings/EntityDetail";
import type { EndpointItem } from "./bindings/EndpointItem";
import type { EndpointSave } from "./bindings/EndpointSave";
import type { EndpointTestReport } from "./bindings/EndpointTestReport";
import type { FirstRunInfo } from "./bindings/FirstRunInfo";
import type { FrameStats } from "./bindings/FrameStats";
import type { HardwareInfo } from "./bindings/HardwareInfo";
import type { IpcError } from "./bindings/IpcError";
import type { MappingView } from "./bindings/MappingView";
import type { ModelRole } from "./bindings/ModelRole";
import type { Occurrence } from "./bindings/Occurrence";
import type { PageView } from "./bindings/PageView";
import type { PatternTest } from "./bindings/PatternTest";
import type { Progress } from "./bindings/Progress";
import type { ProposalCursor } from "./bindings/ProposalCursor";
import type { ProposalsPage } from "./bindings/ProposalsPage";
import type { ProjectEndpoint } from "./bindings/ProjectEndpoint";
import type { ProjectInfo } from "./bindings/ProjectInfo";
import type { QueueWalk } from "./bindings/QueueWalk";
import type { RecentProject } from "./bindings/RecentProject";
import type { RemoveSummary } from "./bindings/RemoveSummary";
import type { ReviewWalk } from "./bindings/ReviewWalk";
import type { RowPage } from "./bindings/RowPage";
import type { RuntimeInfo } from "./bindings/RuntimeInfo";
import type { RowsRequest } from "./bindings/RowsRequest";
import type { SearchCursor } from "./bindings/SearchCursor";
import type { SearchPage } from "./bindings/SearchPage";
import type { SearchQuery } from "./bindings/SearchQuery";
import type { SecretStoreInfo } from "./bindings/SecretStoreInfo";
import type { SegmentHit } from "./bindings/SegmentHit";
import type { Settings } from "./bindings/Settings";
import type { SettingsInfo } from "./bindings/SettingsInfo";
import type { SettingsSaved } from "./bindings/SettingsSaved";
import type { SourceRoot } from "./bindings/SourceRoot";
import type { SplitResult } from "./bindings/SplitResult";
import type { SourceView } from "./bindings/SourceView";
import type { TryResult } from "./bindings/TryResult";
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
  /**
   * Re-does what a settings change affects: reads again the files the OCR languages change
   * (`ocr`), finds identifiers again from the stored text after a region change
   * (`identifiers`). Returns how many files are read again.
   */
  reprocess: (what: { ocr: boolean; identifiers: boolean }) => invoke<number>("reprocess", what),
  sourceRoots: () => invoke<SourceRoot[]>("source_roots"),
  relocateRoot: (root: number, path: string) => invoke<null>("relocate_root", { root, path }),
  removeRoot: (root: number) => invoke<RemoveSummary>("remove_root", { root }),
  customPatterns: () => invoke<CustomPatternItem[]>("custom_patterns"),
  addPattern: (name: string, expression: string) =>
    invoke<null>("add_pattern", { name, expression }),
  editPattern: (name: string, expression: string) =>
    invoke<null>("edit_pattern", { name, expression }),
  removePattern: (name: string) => invoke<null>("remove_pattern", { name }),
  testPattern: (expression: string, sample: string) =>
    invoke<PatternTest>("test_pattern", { expression, sample }),
  mappingView: (refId: number) => invoke<MappingView | null>("mapping_view", { refId }),
  confirmMapping: (refId: number, assignments: ColumnAssignment[]) =>
    invoke<null>("confirm_mapping", { refId, assignments }),
  reuseMapping: (refId: number, fromRefId: number) =>
    invoke<null>("reuse_mapping", { refId, fromRefId }),
  connectionCount: (filter: ConnectionFilter) => invoke<number>("connection_count", { filter }),
  connectionsPage: (filter: ConnectionFilter, cursor: ConnectionCursor | null, limit: number) =>
    invoke<ConnectionsPage>("connections_page", { filter, cursor, limit }),
  /** Counts for the empty states and the "Show ignored" label (US-07.4, US-07.6). */
  connectionsSummary: () => invoke<ConnectionsSummary>("connections_summary"),
  /** Ignores or un-ignores a connection (US-07.4). */
  setIgnored: (key: string, ignored: boolean) => invoke<null>("set_ignored", { key, ignored }),
  /** `includeLow`: also mentions under 0.5 confidence ("Show low confidence", US-10.1 AC3). */
  occurrences: (key: string, includeLow = false) =>
    invoke<Occurrence[]>("occurrences", { key, includeLow }),
  /** How many merge proposals are pending, only the entity's when given (US-11.3, US-07.7 AC4). */
  proposalCount: (entity: string | null) => invoke<number>("proposal_count", { entity }),
  /** A page of pending merge proposals, highest score first (US-11.3 AC1). */
  proposalsPage: (entity: string | null, cursor: ProposalCursor | null, limit: number) =>
    invoke<ProposalsPage>("proposals_page", { entity, cursor, limit }),
  /** Accepts a pending proposal; `null` keeps the name or type it would have (US-11.3 AC2, AC4). */
  proposalAccept: (a: string, b: string, name: string | null, entityType: string | null) =>
    invoke<number>("proposal_accept", { a, b, name, entityType }),
  /** Rejects a pair: never proposed again (US-11.3 AC3). */
  proposalReject: (a: string, b: string) => invoke<number>("proposal_reject", { a, b }),
  /** An entity's page (US-12.1); `null` when no entity holds the key now. */
  entityDetail: (key: string, includeLow = false) =>
    invoke<EntityDetail | null>("entity_detail", { key, includeLow }),
  /** An entity's mentions, at most `limit` and never over 1,000 (US-12.1 AC1, US-11.3 AC1). */
  entityEvidence: (key: string, limit: number, includeLow = false) =>
    invoke<Occurrence[]>("entity_evidence", { key, limit, includeLow }),
  /** Undoes an accepted merge; the pair is stored as rejected (US-11.4 AC1). */
  mergeUndo: (a: string, b: string) => invoke<number>("merge_undo", { a, b }),
  /** Moves picked mentions to a new entity (US-11.4 AC2). */
  entitySplit: (key: string, mentions: number[]) =>
    invoke<SplitResult>("entity_split", { key, mentions }),
  /** Puts a split entity's mentions back. */
  splitUndo: (key: string) => invoke<number>("split_undo", { key }),
  entityRename: (key: string, name: string) => invoke<number>("entity_rename", { key, name }),
  entitySetType: (key: string, entityType: string) =>
    invoke<number>("entity_set_type", { key, entityType }),
  /** Sets an entity's note; an empty one clears it (US-12.2 AC2). */
  entitySetNote: (key: string, note: string) => invoke<number>("entity_set_note", { key, note }),
  aliasRemove: (key: string, alias: string) => invoke<number>("alias_remove", { key, alias }),
  aliasRestore: (key: string, alias: string) => invoke<number>("alias_restore", { key, alias }),
  /** A page of the decision log, newest first, older than `before` (US-11.5 AC2). */
  decisionsPage: (before: number | null, limit: number) =>
    invoke<DecisionsPage>("decisions_page", { before, limit }),
  sourceContext: (mentionId: number) => invoke<SourceView>("source_context", { mentionId }),
  /** A document from its start: an attachment, or the email it came in. */
  sourceDocument: (documentId: number) => invoke<SourceView>("source_document", { documentId }),
  /** Keyword search, best first (US-06.1); a query with no word fails with a plain message. */
  search: (query: SearchQuery, cursor: SearchCursor | null, limit: number) =>
    invoke<SearchPage>("search", { query, cursor, limit }),
  /** The source at a search result, the matched words marked (US-06.1 AC4). */
  searchContext: (hit: SegmentHit) =>
    invoke<SourceView>("search_context", {
      segmentId: hit.segmentId,
      start: hit.start,
      end: hit.end,
    }),
  /** One page of a PDF or image, with the boxes of a mention or a search hit on that page. */
  pageView: (documentId: number, page: number, mentionId: number | null, hit: SegmentHit | null) =>
    invoke<PageView>("page_view", { documentId, page, mentionId, hit }),
  /** The PNG of a page `pageView` has shown, as raw bytes. */
  pageImage: (documentId: number, page: number) =>
    invoke<ArrayBuffer>("page_image", { documentId, page }),
  /** Opens the document's original file with its default app (US-08.6). */
  openOriginal: (documentId: number) => invoke<null>("open_original", { documentId }),
  /** Shows the document's original file in the file manager (US-08.6). */
  showInFolder: (documentId: number) => invoke<null>("show_in_folder", { documentId }),
  /** Opens a link the user confirmed; the core checks it again (US-08.4 AC3). */
  openLink: (url: string) => invoke<null>("open_link", { url }),
  /** Every endpoint with where its host points now (US-09.6, US-09.7 AC4). */
  endpointsList: () => invoke<EndpointItem[]>("endpoints_list"),
  /** Adds or edits an endpoint; a typed key goes to the system keychain only (US-09.6 AC1). */
  endpointSave: (endpoint: EndpointSave) => invoke<EndpointItem>("endpoint_save", { endpoint }),
  endpointDelete: (id: string) => invoke<null>("endpoint_delete", { id }),
  /** Tests an endpoint with fixed texts; sends no project data (US-09.6 AC2/AC3). */
  endpointTest: (id: string) => invoke<EndpointTestReport>("endpoint_test", { id }),
  /** Where a base URL points now: host, class, addresses. */
  resolveDestination: (baseUrl: string) => invoke<Destination>("resolve_destination", { baseUrl }),
  secretStoreStatus: () => invoke<SecretStoreInfo>("secret_store_status"),
  /** The open project's permission for every endpoint (US-09.7). */
  projectEndpoints: () => invoke<ProjectEndpoint[]>("project_endpoints"),
  /** What the "Allow for this project" dialog shows, resolved now (US-09.7 AC2). */
  endpointAllowPrepare: (id: string) => invoke<AllowPrompt>("endpoint_allow_prepare", { id }),
  /** Stores the permission for exactly this origin and class; fails if the host moved. */
  endpointAllowConfirm: (id: string, origin: string, cls: DestinationClass) =>
    invoke<null>("endpoint_allow_confirm", { id, origin, class: cls }),
  endpointRevoke: (id: string) => invoke<null>("endpoint_revoke", { id }),
  /** "Data sent from this project" (US-09.8 AC2). */
  egressRecords: () => invoke<EgressRecord[]>("egress_records"),
  /** The hardware check (US-09.1); `recheck` reads the machine again ("Check hardware"). */
  hardwareCheck: (recheck: boolean) => invoke<HardwareInfo>("hardware_check", { recheck }),
  /** The shipped catalog with each model's state and progress (US-09.3, US-09.4). */
  modelCatalog: () => invoke<CatalogModel[]>("model_catalog"),
  /** Starts a download; refused with the reason below the minimum or without the disk space. */
  modelDownloadStart: (id: string) => invoke<null>("model_download_start", { id }),
  modelDownloadPause: (id: string) => invoke<null>("model_download_pause", { id }),
  modelDownloadResume: (id: string) => invoke<null>("model_download_resume", { id }),
  /** Cancels a download and deletes its partial files. */
  modelDownloadCancel: (id: string) => invoke<null>("model_download_cancel", { id }),
  /** Deletes an installed model (US-09.4 AC5). */
  modelDelete: (id: string) => invoke<null>("model_delete", { id }),
  /** US-09.2 AC3 (Q7): allow local models below the minimum. */
  localModelsOverride: (on: boolean) => invoke<HardwareInfo>("local_models_override", { on }),
  /** The llama-server in use, its build and backend (ADR-0015); `recheck` finds it again. */
  runtimeStatus: (recheck: boolean) => invoke<RuntimeInfo>("runtime_status", { recheck }),
  /** "Choose…": the llama-server to use; `null` clears the setting. */
  runtimeSetPath: (path: string | null) => invoke<RuntimeInfo>("runtime_set_path", { path }),
  /** "Retry" after a failure: checks again and starts failed models again. */
  runtimeRetry: () => invoke<RuntimeInfo>("runtime_retry"),
  /** "Try": starts an installed model on 127.0.0.1 and sends the fixed test texts. */
  modelTry: (id: string) => invoke<TryResult>("model_try", { id }),
  /** What each role uses (both roles). */
  aiChoice: () => invoke<AiChoiceInfo[]>("ai_choice"),
  /** "Use for AI"; `null` goes back to the default. */
  aiChoiceSet: (role: ModelRole, choice: AiChoice | null) =>
    invoke<AiChoiceInfo[]>("ai_choice_set", { role, choice }),
  /** Whether each role's AI can run for the open project (US-09.7 AC3). */
  aiReadiness: () => invoke<AiReadinessInfo[]>("ai_readiness"),
  /** Where data goes now, if an AI call is on its way (US-09.8 AC1). */
  aiActivity: () => invoke<AiActivityInfo | null>("ai_activity"),
  /** What AI extraction is doing (US-10.2 AC2, US-10.3, US-10.4). */
  aiExtractionStatus: () => invoke<AiExtractionInfo>("ai_extraction_status"),
  /** How much `scope` would be, to show before it starts (gap A3). */
  aiExtractionEstimate: (scope: AiScope) => invoke<AiEstimate>("ai_extraction_estimate", { scope }),
  /** "Find people and organizations" for the picked files, or all (gap A3). */
  aiExtractionRun: (scope: AiScope) => invoke<null>("ai_extraction_run", { scope }),
  /** "Re-run with Y": nothing visible changes until it is done (US-10.4, ADR-0005). */
  aiExtractionRerun: () => invoke<AiEstimate>("ai_extraction_rerun"),
  /** Cancels the re-run, or else the rest of the run. False when nothing was running. */
  aiExtractionCancel: () => invoke<boolean>("ai_extraction_cancel"),
  /** What meaning search can do now, and why not (US-06.2 AC2). */
  meaningStatus: () => invoke<MeaningStatus>("meaning_status"),
  /** One meaning query: the top 20, or why it cannot run (US-06.2 AC1, AC2). */
  meaningSearch: (query: string) => invoke<MeaningResult>("meaning_search", { query }),
  /** Starts everything not embedded yet, cancelled and failed parts too. */
  meaningRun: () => invoke<null>("meaning_run"),
  /** "Re-run with Y": the vectors stay until it is done (ADR-0005). */
  meaningRerun: () => invoke<MeaningEstimate>("meaning_rerun"),
  /** Cancels the re-run, or else the rest of the run. False when nothing was running. */
  meaningCancel: () => invoke<boolean>("meaning_cancel"),
  /** Whether to show the first-run screen, and where the demo files are (US-14.4). */
  firstRun: () => invoke<FirstRunInfo>("first_run"),
  firstRunFinish: () => invoke<null>("first_run_finish"),
  /** Opens the demo project with the demo files at `path`, or where they were found. */
  demoOpen: (path: string | null) => invoke<ProjectInfo>("demo_open", { path }),
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
  /** `MK_REVIEW_WALK=1` only. Saves the walk and exits the app. */
  recordReviewWalk: (walk: ReviewWalk) => invoke<string>("record_review_walk", { walk }),
  /** Spike S2d builds only. */
  syntheticRows: (request: RowsRequest) => invoke<RowPage>("synthetic_rows", { request }),
  /** Spike S2d builds only. Returns the path the stats were written to. */
  recordFrameStats: (stats: FrameStats) => invoke<string>("record_frame_stats", { stats }),
};

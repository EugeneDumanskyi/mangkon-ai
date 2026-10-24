# ADR-0003: Desktop shell and UI — Tauri 2 + React + TypeScript

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-SEC-09/10, NFR-PERF-03/04, NFR-RES-07, NFR-USE-04/06/07/08/09, NFR-MNT-08, ADR-0002, ADR-0011

## Context

- The NFRs already name Tauri (NFR-SEC-10). This ADR records why it still holds and picks the UI stack inside it.
- The UI must scroll 100,000-row lists at 60 fps (NFR-PERF-04). It must be fully usable by keyboard (NFR-USE-06), support light and dark themes (NFR-USE-08), and meet WCAG AA contrast (NFR-USE-09).
- The app binary must stay ≤ 150 MB without models (NFR-RES-07).
- The webview shows untrusted content (emails, HTML). It must load no remote URLs and run no document scripts (NFR-SEC-09/10).
- The code is open source. A popular stack helps contributors.

## Options considered

**Shell**

| Option | ➕ | ➖ |
|---|---|---|
| **Tauri 2** | Small binary (system webview). Rust core in the same process. Fine-grained IPC permissions (capabilities). | Webviews differ by OS (WebKit on macOS/Linux, WebView2 on Windows). Must test on all three. |
| Electron | One browser engine everywhere. Huge ecosystem. | Bundles Chromium (+100 MB or more). Node.js in the app adds attack surface. Rust core would need a separate process anyway. |
| Native Rust UI (egui, Slint) | No webview. Very small. | Rich text, PDF page view, and accessible virtualized tables take much more work. Fewer contributors. |

**UI framework (inside Tauri)**

| Option | ➕ | ➖ |
|---|---|---|
| **React 19 + TypeScript** | Largest ecosystem: TanStack Virtual / Table / Query, accessible component kits. Most contributors. | Larger runtime than Svelte or Solid. Not a real issue for a desktop app. |
| Svelte 5 | Small, fast, little boilerplate. | Smaller ecosystem for virtualized grids and accessible components. |
| SolidJS | Very fast fine-grained updates. | Smallest ecosystem and contributor pool. |

## Decision

- **Shell:** Tauri 2.
- **UI:** React 19, TypeScript in strict mode, Vite.
- **Lists and grids:** TanStack Virtual (and TanStack Table where columns are needed). The UI asks the core for **pages of rows** (keyset paging). It never loads 100,000 rows at once.
- **Data fetching:** TanStack Query over Tauri IPC. Core events only say "X changed"; the UI re-queries (05-architecture §7).
- **Typed IPC: `ts-rs` 12** (picked in M2a). Command argument and result types live in `apps/desktop/src/ipc.rs` and derive `TS`; `cargo test` writes them to `ui/src/ipc/bindings/`. The files are committed, and CI fails if they differ from what the Rust types give (`cargo xtask check-bindings`). `ui/src/ipc/commands.ts` is the only module that calls `invoke`, typed against the bindings; an ESLint rule blocks `@tauri-apps/*` imports anywhere else. No hand-written duplicate types. Why not `tauri-specta`: it is still 2.0 release candidate, and the core path takes no pre-release dependency. `ts-rs` is stable, needs no Tauri plugin, and exports plain types; the command names in `commands.ts` are typed by hand (one line each).
- **Styling:** Tailwind v4 (brief §14) over CSS variables. Theme tokens are CSS variables in `ui/src/index.css`, switched by `prefers-color-scheme`, so light/dark follow the OS setting (NFR-USE-08). Components use the token names (`bg-surface`, `text-muted`), never raw colours. Components are chosen for keyboard and screen-reader support.
- **Security settings** (checked against Tauri 2.12 in M2a: the config passes `tauri-build`'s schema check, and the S2d release runs loaded styles, scripts and IPC under this CSP):
  - CSP, in `apps/desktop/tauri.conf.json`: `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data: blob:; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'`. Tauri adds hashes for its own inline scripts and styles. `ipc:` is the IPC scheme on Linux and macOS, `http://ipc.localhost` on Windows. React sets inline styles through the CSSOM, which the CSP does not block.
  - `devCsp` (dev builds only, with the Vite dev server): the same, plus `http://localhost:5173` and `ws://localhost:5173`, and `'unsafe-inline'` for scripts and styles, which Vite's hot reload needs. Never in a release build.
  - `freezePrototype: true`; the asset protocol is off; `withGlobalTauri: false`.
  - Tauri capabilities: only Mangkon's own IPC commands. `build.rs` lists the commands, so each gets a generated permission, and `capabilities/main.json` grants exactly those. No `core:` permissions and **no** `fs`, `http`, `shell`, or `opener` plugins. The `dynamic-acl` feature is off, so capabilities cannot be added at run time. Spike commands sit in their own capability file, loaded only with feature `spike-s2d`.
  - The app window never navigates to a remote URL: an `on_navigation` guard allows only the app's own origin (`tauri://localhost`, `http(s)://tauri.localhost`; the dev server only in dev builds), and new windows are refused. External links open in the system browser only after the user clicks and confirms (from M3).
  - App identifier `org.mangkon.desktop`. ASSUMPTION: no domain is registered; it decides the OS app-data folder name, so it should be fixed before the first release.

## Consequences

**Easier**
- Small binary. Most of the 150 MB budget is left for parser and OCR libraries.
- The UI cannot reach the network by itself. All traffic goes through the core (ADR-0009).
- Many contributors can work on the UI.

**Harder**
- Three webview engines. CI must run UI smoke tests on macOS, Windows, and Linux (NFR-MNT-02).
- WebKitGTK on Linux is the slowest. Test NFR-PERF-04 there first.

**Follow-ups**
- M2 (spike S2d): scroll test with 100,000 rows on WebKitGTK. **Done in M2a** (`10-walking-skeleton.md` §S2d): 60 fps on average (mean frame 16.7 ms, equal to the idle control), p95 18 ms, 0.03–0.24 % of frames over 25 ms. WebKitGTK reports frame times in whole milliseconds, so p95 ≤ 16.7 ms is not reachable even when idle; the pass mark is read for a 1 ms clock, pending approval. macOS and Windows not run yet.
- Pick the TypeScript type-export tool in M2. **Done in M2a:** `ts-rs` (Decision above).

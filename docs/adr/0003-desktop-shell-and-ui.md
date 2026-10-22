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
- **Typed IPC:** Rust command types are exported to TypeScript (for example with `tauri-specta` or `ts-rs`; pick in M2). No hand-written duplicate types.
- **Styling:** CSS variables for light/dark themes that follow the OS setting. Components are chosen for keyboard and screen-reader support.
- **Security settings:**
  - CSP: `default-src 'self'; img-src 'self' data: blob:; connect-src ipc: http://ipc.localhost; script-src 'self'; object-src 'none'; frame-src 'none'`. ASSUMPTION — final values checked against Tauri 2 docs in M2.
  - Tauri capabilities: only Mangkon's own IPC commands. **No** `fs`, `http`, `shell`, or `opener` plugins exposed to the webview.
  - The app window never navigates to a remote URL. External links open in the system browser only after the user clicks and confirms.

## Consequences

**Easier**
- Small binary. Most of the 150 MB budget is left for parser and OCR libraries.
- The UI cannot reach the network by itself. All traffic goes through the core (ADR-0009).
- Many contributors can work on the UI.

**Harder**
- Three webview engines. CI must run UI smoke tests on macOS, Windows, and Linux (NFR-MNT-02).
- WebKitGTK on Linux is the slowest. Test NFR-PERF-04 there first.

**Follow-ups**
- M2 (spike S2d): scroll test with 100,000 rows on WebKitGTK.
- Pick the TypeScript type-export tool in M2.

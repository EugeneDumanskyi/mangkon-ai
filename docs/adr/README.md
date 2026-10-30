# Architecture decision records

Each file records one significant decision for Mangkon AI. The big picture is in [`../05-architecture.md`](../05-architecture.md).

## Format

Every ADR has the same sections:

- **Context** — the problem and the forces (NFRs, invariants, constraints).
- **Options considered** — the real alternatives, with their main pros and cons.
- **Decision** — what we do, specific enough to build.
- **Consequences** — what gets easier, what gets harder, and follow-ups.

Header fields: **Status** (Proposed · Accepted · Superseded by ADR-xxxx · Open), **Date**, **Related** (NFR IDs, invariant IDs, other ADRs).

## Rules

- Numbers are never reused. A changed decision gets a **new** ADR that supersedes the old one. The old one stays.
- `ASSUMPTION:` marks a guess that needs confirmation, same as in the other docs.
- Status moves from Proposed to Accepted when the maintainer approves it in review.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-architecture-style.md) | Architecture style: modular monolith with isolated parser workers | Accepted |
| [0002](0002-headless-core-workspace.md) | Headless Rust core in a Cargo workspace | Accepted |
| [0003](0003-desktop-shell-and-ui.md) | Desktop shell and UI: Tauri 2 + React + TypeScript | Accepted |
| [0004](0004-project-storage.md) | Storage: one SQLite file per project, WAL, single writer | Accepted |
| [0005](0005-derived-data-and-decisions.md) | Derived data and decisions: rebuild in the same commit | Accepted |
| [0006](0006-background-jobs.md) | Background jobs: persistent queue in the project store | Accepted |
| [0007](0007-parser-worker-isolation.md) | Parser isolation: worker processes with time and memory limits | Accepted |
| [0008](0008-local-model-runtime.md) | Local model runtime: `llama-server` sidecar from pinned releases | Superseded by ADR-0015 |
| [0009](0009-egress-gate.md) | One egress gate for all model traffic | Proposed |
| [0010](0010-search.md) | Search: FTS5 + binary vector index with rescoring | Proposed |
| [0011](0011-source-viewer.md) | Source viewer: stored text first, page view from the same engine | Accepted |
| [0012](0012-secrets.md) | API keys in the OS secret store | Proposed |
| [0013](0013-parsing-and-ocr-engines.md) | Parsing and OCR engines | Superseded by ADR-0014 |
| [0014](0014-pdf-and-ocr-engines.md) | PDF and OCR engines: hayro and Tesseract | Accepted |
| [0015](0015-user-installed-llama-server.md) | Local model runtime: user-installed `llama-server` | Accepted |
| [0016](0016-streamed-file-limits.md) | Streamed files: deadline per unit of progress, staged commit | Accepted |
| [0017](0017-paged-file-deadlines.md) | PDFs and images: deadline per page, 30 s per OCR page | Accepted |
| [0018](0018-ocr-language-files.md) | OCR language files: English built in, others installed by the user | Accepted |

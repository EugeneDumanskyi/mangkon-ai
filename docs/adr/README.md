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
| [0001](0001-architecture-style.md) | Architecture style: modular monolith with isolated parser workers | Proposed |
| [0002](0002-headless-core-workspace.md) | Headless Rust core in a Cargo workspace | Proposed |
| [0003](0003-desktop-shell-and-ui.md) | Desktop shell and UI: Tauri 2 + React + TypeScript | Proposed |
| [0004](0004-project-storage.md) | Storage: one SQLite file per project, WAL, single writer | Proposed |
| [0005](0005-derived-data-and-decisions.md) | Derived data and decisions: rebuild in the same commit | Proposed |
| [0006](0006-background-jobs.md) | Background jobs: persistent queue in the project store | Proposed |
| [0007](0007-parser-worker-isolation.md) | Parser isolation: worker processes with time and memory limits | Proposed |
| [0008](0008-local-model-runtime.md) | Local model runtime: `llama-server` sidecar from pinned releases | Proposed |
| [0009](0009-egress-gate.md) | One egress gate for all model traffic | Proposed |
| [0010](0010-search.md) | Search: FTS5 + binary vector index with rescoring | Proposed |
| [0011](0011-source-viewer.md) | Source viewer: stored text first, page view from the same engine | Proposed |
| [0012](0012-secrets.md) | API keys in the OS secret store | Proposed |
| [0013](0013-parsing-and-ocr-engines.md) | Parsing and OCR engines | Open — decided in M0 |

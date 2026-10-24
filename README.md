# Mangkon AI

A free, open-source, local-first desktop app for finding hidden connections across mixed files.

Drop in a folder of PDFs, scans, emails (EML/MBOX), CSV, JSON and DOCX files. Mangkon AI finds the identifiers, people and organizations they share, and shows how they connect across sources. Every link opens the exact spot in the source file. It runs on your machine; project data goes out only to a model endpoint you set yourself.

## Status

**Design phase. Nothing is built yet.** There is no release and no build to try.

## Design docs

| Doc | Content |
|---|---|
| [Brief](docs/01-brief.md) | Scope, layers, milestones, tech decisions |
| [Requirements](docs/02-requirements.md) | Epics and user stories with acceptance criteria |
| [Non-functional requirements](docs/03-nfr.md) | Performance, security, reliability targets; reference machines; test data |
| [Domain model](docs/04-domain.md) | Bounded contexts, entities, invariants, state machines |
| [Architecture](docs/05-architecture.md) | The big picture, with decisions recorded in [ADRs](docs/adr/README.md) |
| [Roadmap](docs/06-roadmap.md) | Milestones M0–M9, spikes, risk burn-down |
| [Test corpus](docs/07-test-corpus.md) | Test data story, files, ground-truth format, broken-file set |
| [Engine bake-off](docs/08-engine-bakeoff.md) | PDF and OCR engine measurements (spike S0), the `Locator` design |
| [Model check](docs/09-model-check.md) | Local model quality and speed, embeddings with binary vectors, hardware tiers (spike S1) |
| [Install llama.cpp](docs/install-llama-cpp.md) | The `llama-server` prerequisite: minimum build, install options per OS |

## Prerequisites

Local AI needs `llama-server` from llama.cpp, build 10863 or newer, installed by you ([install guide](docs/install-llama-cpp.md)). The app finds it and checks its version; it never downloads it. Without it, everything except AI extraction and meaning search still works.

## Planned stack

Tauri 2 desktop shell, React + TypeScript UI, Rust core. One SQLite file per project. Local models through `llama-server` (llama.cpp), or any OpenAI-compatible endpoint you set. No telemetry.

## License

[AGPL-3.0](LICENSE).

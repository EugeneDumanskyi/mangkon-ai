# Mangkon AI

A free, open-source, local-first desktop app for finding hidden connections across mixed files.

Drop in a folder of PDFs, scans, emails (EML/MBOX), CSV, JSON and DOCX files. Mangkon AI finds the identifiers, people and organizations they share, and shows how they connect across sources. Every link opens the exact spot in the source file. It runs on your machine; project data goes out only to a model endpoint you set yourself.

## Status

**In development, no release yet.** You can build it from source and use it on real folders; it has been tested on Linux only. Finding identifiers, keyword search, AI extraction of people and organizations, meaning search, the review queue for merges, and entity pages all work.

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
| [Walking skeleton](docs/10-walking-skeleton.md) | Build time, app size, and the 100,000-row scroll test (M2 spikes) |
| [Ingestion](docs/11-ingestion.md) | Streaming, PDF, OCR and page view measurements (M3) |
| [Layer 1](docs/12-layer1.md) | Keyword search measurements (M4) |
| [Model manager](docs/13-model-manager.md) | Egress gate, endpoints, downloads and the runtime supervisor (M5) |
| [AI extraction](docs/14-ai-extraction.md) | Stage B, kill tests and real runs (M6a) |
| [Meaning search](docs/15-meaning-search.md) | Search latency and recall at scale (M6b) |
| [Entities and acceptance](docs/16-entities-acceptance.md) | Entities on the connections screen, the LLM benchmark, M6 results (M6c) |
| [Entity resolution](docs/17-entity-resolution.md) | Merge proposals, decisions that survive re-runs, memory, review queue, M7 results |
| [Install llama.cpp](docs/install-llama-cpp.md) | The `llama-server` prerequisite: minimum build, install options per OS |
| [Add OCR languages](docs/install-ocr-languages.md) | English is built in; where other Tesseract language files go |

## Prerequisites

Local AI needs `llama-server` from llama.cpp, build 10863 or newer, installed by you ([install guide](docs/install-llama-cpp.md)). The app finds it and checks its version; it never downloads it. Without it, everything except AI extraction and meaning search still works.

OCR reads English out of the box. For other languages, copy Tesseract's language files into the app's folder ([how](docs/install-ocr-languages.md)); the app never downloads them.

## Build from source

One command builds the app: `cargo xtask build --release`. It first checks the prerequisites below and says which one is missing. It needs no Python and no Docker. The build downloads the Tesseract and Leptonica sources and the English OCR data with `curl`, checks their SHA-256 against `crates/mk-tesseract-sys/native/sources.toml`, and compiles them into the app.

**Prerequisites, every OS**

- Rust through [rustup](https://rustup.rs). The repo pins the toolchain in `rust-toolchain.toml`; rustup installs it on first use.
- [Node.js](https://nodejs.org/en/download) 20.19+ or 22.12+, with npm.
- CMake 3.18+, a C++ compiler, and `curl`.

**Linux** (tested on Ubuntu 24.04; Ubuntu 22.04 is the oldest supported). Node from your distribution may be too old; use the nodejs.org build or a version manager.

```sh
sudo apt install build-essential cmake curl pkg-config \
  libwebkit2gtk-4.1-dev libsoup-3.0-dev librsvg2-dev libdbus-1-dev
```

Other distributions (not tested): Fedora `sudo dnf install gcc-c++ cmake curl webkit2gtk4.1-devel libsoup3-devel librsvg2-devel`; Arch `sudo pacman -S base-devel cmake curl webkit2gtk-4.1 libsoup3 librsvg`.

**macOS 13+** (written, not yet tested): `xcode-select --install`, then `brew install cmake node`.

**Windows 10/11** (untested, contributions welcome): [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with "Desktop development with C++", then `winget install Kitware.CMake OpenJS.NodeJS.LTS Rustlang.Rustup`. `curl` and the WebView2 runtime come with Windows.

**Build and run**

```sh
git clone <this repository> && cd mangkon-ai
cargo xtask build --release
target/release/mangkon            # target\release\mangkon.exe on Windows
```

Click **New project…** and pick an empty folder, then add a folder (button or drag and drop). Connections are identifiers, people or organizations found in two or more sources; click one, then a mention, to see it in its file.

The same from the command line:

```sh
cargo build --release -p mangkon-cli
target/release/mangkon-cli ingest --project ~/mangkon/p1 ~/Documents/case
target/release/mangkon-cli rebuild --project ~/mangkon/p1
```

- Offline build: put the files listed in `crates/mk-tesseract-sys/native/sources.toml` in a folder and set `MK_NATIVE_SRC_DIR` to it. Nothing is downloaded then.
- All checks, in the same order as CI: `cargo xtask ci` (needs `cargo install --locked cargo-deny` once).
- UI work with hot reload: `npm run dev` in `ui/`, and in another terminal `cargo run -p mangkon-desktop --no-default-features`.

## Stack

Tauri 2 desktop shell, React + TypeScript UI, Rust core. Tesseract compiled in for OCR. One SQLite file per project. Local models through `llama-server` (llama.cpp), or any OpenAI-compatible endpoint you set. No telemetry.

## License

[AGPL-3.0](LICENSE).

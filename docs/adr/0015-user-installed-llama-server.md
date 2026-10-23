# ADR-0015: Local model runtime: user-installed `llama-server`

- **Status:** Accepted
- **Date:** 2026-10-23
- **Related:** supersedes ADR-0008; NFR-SEC-02/06/11, NFR-REL-08/12, NFR-RES-06/07, NFR-COMP-03/04/05, NFR-USE-01/03, NFR-MNT-01/09, NFR-LEG-05, INV-MM-01..06, domain §7.7, §7.8, risk R3, gap A1, ADR-0009

## Context

- The brief picks llama.cpp for local models, with models auto-downloaded from Hugging Face. The NFRs name `llama-server` (NFR-REL-12, NFR-SEC-06).
- ADR-0008 (Proposed) had the app download a pinned prebuilt `llama-server` per OS and GPU backend and verify its SHA-256. That puts a second download host next to Hugging Face (gap A1) and a binary catalog into the app.
- The R3 desk check (`08-engine-bakeoff.md` §16) found prebuilt llama.cpp builds for every desktop OS: CPU, Vulkan, CUDA 12/13, Metal, ROCm and more. llama.cpp is also packaged for common package managers.
- Every llama.cpp build is now a pre-release; `vX.Y.Z` releases name the build they stand for. llama.cpp changes fast.
- The user's build stays one command with no GPU toolchain (NFR-MNT-01, NFR-USE-03).
- NFR-SEC-02: network use only for model download hosts and user-set endpoints.
- Any OpenAI-compatible endpoint can be used instead (NFR-COMP-05).

## Options considered

**1. The app downloads a pinned prebuilt `llama-server`** (ADR-0008).
- ➕ Nothing to install by hand. The app picks the backend for the hardware.
- ➖ A second download host (`github.com` → `release-assets.githubusercontent.com`) or a mirror to maintain (gap A1).
- ➖ The app keeps a binary catalog per OS × arch × backend, bumps it on purpose, and ships llama.cpp's notices.
- ➖ CUDA builds need a CUDA runtime archive of 440–594 MB on top.

**2. `llama-server` is a prerequisite the user installs; the app finds it and checks its version.**
- ➕ The app downloads no binaries. Hugging Face stays the only download host; gap A1 closes.
- ➕ The user picks the build for their GPU, or uses one already installed. No binary catalog in the app.
- ➕ The user can update llama.cpp without an app release.
- ➖ One more install step before the first local AI run.
- ➖ GPU support depends on the build the user installs. A wrong build (CPU only on a GPU machine) is slow, not broken.
- ➖ The app must cope with versions it was not tested with: a minimum version, checked at start.

**3. Link llama.cpp into the app, or build `llama-server` from source in the app's build.**
- ➕ No install step and no download.
- ➖ The user's build needs the CUDA or Metal toolchain and takes long (breaks NFR-USE-03 on many machines). A driver crash can take the app down (option 3 of ADR-0008).

**4. Only external runtimes (Ollama, LM Studio).**
- ➖ No model auto-download that fits the machine. Two more products between the user and the model.

## Decision

Use **option 2**.

**Finding the runtime**
- Order: the path set in the app settings, then `llama-server` on `PATH`. No other search.
- In Starting (domain §7.8), before the process is launched, run `llama-server --version` and read the build number. ASSUMPTION: the output holds `version: <build> (<commit>)`, as current builds print. Checked in M1.
- Not found → Starting goes to Failed with "llama-server not found", and a link to the install guide. Build number below the minimum → Failed with "llama.cpp build <n> found; build <min> or newer is needed", and the same link. As for any Failed, AI jobs become Blocked with the reason and Layer 1 keeps working (US-09.5 AC3, ADR-0006). "User retries" checks again, so installing llama.cpp needs no app restart.
- The minimum build is set per app version, from the build M1 tests with. ASSUMPTION until M1: `b11669` (the build named by llama.cpp `v0.7.0`).
- The app never downloads, updates or patches `llama-server`.

**Install guide** (in the README and the docs, written in M1)
- Minimum version, and how to check it: `llama-server --version`.
- Install options per OS: a package manager (for example Homebrew `llama.cpp`), or a prebuilt archive from the llama.cpp releases page, with which archive to pick per GPU: CUDA (NVIDIA), Metal (Apple, in the macOS arm64 build), Vulkan (AMD, Intel, and the smaller NVIDIA option). ASSUMPTION: package names per OS are checked when the guide is written.
- The hardware check (BC-12) suggests a backend when it sees a GPU and a CPU-only build.

**Supervisor and client** (unchanged from ADR-0008)
- The supervisor follows domain §7.8. Every app start begins at Stopped.
- Start: `llama-server --host 127.0.0.1 --port <free port> --api-key <random per start> -m <model> …` (NFR-SEC-06). Health check on `/health` before Ready. One process per loaded model.
- Idle 10 min → stop (NFR-RES-06). Crash → auto-restart, at most 2 times (NFR-REL-12, INV-MM-06).
- One OpenAI-compatible client in `mk-egress` for local and external endpoints, created only by the egress gate (ADR-0009). JSON-schema `response_format` when supported.

**Models**
- Models are still downloaded from Hugging Face: resumable, SHA-256-checked against the catalog, disk space checked first (NFR-REL-08, NFR-SEC-11, INV-MM-01/02). The catalog (NFR-MNT-09) lists models and hardware tiers, not runtime binaries.

## Consequences

**Easier**
- Gap A1 is decided: the app downloads no binaries at run time. NFR-SEC-02 needs no new host.
- No binary catalog, no per-backend download code, no llama.cpp notices in `THIRD_PARTY_NOTICES` (llama.cpp is not shipped).
- Risk R3 is no longer the app's risk. It becomes install guidance.

**Harder**
- `git clone` → first result (NFR-USE-01) now includes installing llama.cpp. ASSUMPTION: a few minutes with a package manager; measured in M1.
- GPU support (NFR-COMP-03/04) depends on the user's llama.cpp build. The app can only detect and explain.
- Newer llama.cpp builds can change flags or output. The minimum version and the start command are tested in CI against the pinned minimum build, and newer builds are tried by hand.

**Follow-ups**
- M1: run `llama-server` from `PATH` in the runtime harness; confirm the `--version` format, `--api-key` and `/health`; set the minimum build; write the install guide.
- M1: update NFR wording where it assumes a downloaded runtime: NFR-SEC-11 (binaries), NFR-LEG-05 (llama.cpp in notices), NFR-USE-01 (install step in the timed run).
- M5: the model manager shows the found `llama-server` path, its build and the backend, with the reason when it is unusable.

# ADR-0008: Local model runtime — `llama-server` sidecar from pinned releases

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-REL-08/12, NFR-RES-04/05/06, NFR-SEC-02/06/11, NFR-COMP-03/04/05, NFR-USE-01/03, NFR-MNT-01, NFR-LEG-05, INV-MM-01..06, domain §7.7, §7.8, ADR-0009

## Context

- The brief picks llama.cpp for local models. The NFRs name `llama-server` (NFR-REL-12).
- Users **build from source**: one command, no Python, no Docker, ≤ 15 min (NFR-MNT-01, NFR-USE-03).
- GPU support: NVIDIA (CUDA) and Apple (Metal) full; AMD/Intel (Vulkan) best effort (NFR-COMP-03).
- `git clone` → first cross-source result in < 30 min (NFR-USE-01).
- A runtime crash must not stop Layer 1 work. Auto-restart at most 2 times (NFR-REL-12).
- The runtime listens on `127.0.0.1` only, on a random free port (NFR-SEC-06).
- Any OpenAI-compatible endpoint can be used instead (NFR-COMP-05).

## Options considered

**1. Link llama.cpp into the app (Rust bindings such as `llama-cpp-2`).**
- ➕ No extra process. No port.
- ➖ The user's build compiles llama.cpp with CUDA or Metal. CUDA needs the CUDA toolkit and takes a long time. Breaks NFR-USE-03 and NFR-MNT-01 on many machines.
- ➖ A GPU driver crash kills the whole app.

**2. `llama-server` sidecar, prebuilt binary downloaded from a pinned llama.cpp release, checksum-verified.**
- ➕ No GPU toolchain in the user's build.
- ➕ A crash only stops the sidecar. Layer 1 continues.
- ➕ The same OpenAI-compatible client serves local and external models. One code path.
- ➖ A download step at first run. One more network host (gap A1).
- ➖ A prebuilt binary may not exist for every OS/GPU pair (risk R3).

**3. `llama-server` sidecar built from source by the app's build script.**
- ➕ Works on any platform llama.cpp supports.
- ➖ Same toolchain and build-time problems as option 1.

**4. Require the user to install Ollama or LM Studio.**
- ➕ No runtime work for us.
- ➖ Breaks the "auto-download a model that fits your machine" promise and the 30-minute first-result goal.

## Decision

Use **option 2**, with **option 3 as a documented fallback** for platforms without a prebuilt binary.

**Catalog**
- The model catalog (NFR-MNT-09) also lists runtime binaries: llama.cpp release tag, OS, architecture, backend (`cuda`, `metal`, `vulkan`, `cpu`), URL, size, SHA-256.
- The hardware check (BC-12) picks the backend. A binary is used only after its checksum matches (INV-MM-01, NFR-SEC-11).
- Downloads resume after network loss (NFR-REL-08) and check free disk first (INV-MM-02).
- The release tag is pinned per app version. Updating it is a normal PR with a benchmark run (NFR-MNT-06).

**Supervisor (in `mk-models`)**
- Follows domain §7.8 exactly. The state is never saved; every app start begins at Stopped.
- Start: `llama-server --host 127.0.0.1 --port <free port> --api-key <random per start> -m <model> …`. If the port is taken in the meantime, retry with a new port.
- The random API key is cheap hardening: other local programs cannot use the runtime by accident. ASSUMPTION — keep if the llama-server version supports it for all used routes.
- One process per loaded model: one for extraction, one for embeddings (embedding mode).
- Health check on `/health` before Ready.
- Idle 10 min → stop (NFR-RES-06).
- Crash → auto-restart, at most 2 times. The counter resets after a successful AI run (INV-MM-06). After that: Failed, and AI jobs become Blocked with the reason.

**Client**
- One OpenAI-compatible client in `mk-egress` for `/v1/chat/completions` and `/v1/embeddings`. It is used for the local runtime and for external endpoints. It can only be created by the egress gate (ADR-0009).
- Structured output: use JSON-schema `response_format` when the server supports it (llama-server does). Otherwise, prompt for JSON and validate (INV-AI-03). The endpoint's `structuredOutput` flag records which.

## Consequences

**Easier**
- The user's build stays pure Rust + Node. No CUDA toolkit needed.
- Local and external models share one client and one test suite (NFR-COMP-05).
- GPU or driver problems cannot crash the app.

**Harder**
- NFR-SEC-02 must allow the release host, or the binaries must be mirrored to Hugging Face (gap A1).
- `THIRD_PARTY_NOTICES` must include llama.cpp and its bundled libraries (NFR-LEG-05).
- llama.cpp moves fast. Pinned versions must be bumped on purpose.

**Follow-ups**
- M0: list which OS/backend pairs have official prebuilt binaries at the pinned tag (risk R3). Write the fallback build guide for the rest.
- Decide gap A1 with the maintainer.

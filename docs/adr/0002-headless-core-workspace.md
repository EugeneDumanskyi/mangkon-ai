# ADR-0002: Headless Rust core in a Cargo workspace

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-MNT-02/04/05/08, NFR-USE-03, US-14.3, domain §3 (bounded contexts), ADR-0001

## Context

- The Layer 1 benchmark must run on **every PR**, without an LLM and without a GUI (NFR-MNT-05).
- Rust test coverage must be ≥ 70% (NFR-MNT-04). That is hard if logic sits inside Tauri command handlers.
- The domain has 13 bounded contexts with clear boundaries (domain §3). The code should keep those boundaries visible.
- Parser workers (ADR-0007) need the parsing and identifier code, but not the rest of the core.
- Later versions may want a CLI or a service (ADR-0001).

## Options considered

**1. All logic inside the Tauri app crate.**
- ➕ Fastest start.
- ➖ Tests and the benchmark need a Tauri runtime. Hard in CI.
- ➖ Context boundaries blur over time.

**2. Headless core crates + thin Tauri shell + CLI, in one Cargo workspace.**
- ➕ The benchmark, tests, and kill tests run without a GUI.
- ➕ Crate boundaries enforce context boundaries. The compiler stops wrong dependencies.
- ➕ Workers link only the crates they need.
- ➖ More crates to set up. Some boilerplate for ports (traits).

**3. Separate repositories or published libraries per context.**
- ➕ Strongest separation.
- ➖ Too much overhead for a project this size. Version juggling.

## Decision

Use **option 2**: one Cargo workspace, headless core, thin entry points.

```text
mangkon-ai/
  crates/
    mk-kernel       shared kernel: Locator, Span, ContentHash, keys, Mention (no I/O)
    mk-workspace    BC-01
    mk-corpus       BC-02 model, re-scan, presence
    mk-parsers      BC-02 parsers + OCR adapters (used by workers)
    mk-worker       worker protocol, framing and loop: runs in worker processes (ADR-0007)
    mk-tesseract-sys  Tesseract + Leptonica from pinned sources, minimal FFI (ADR-0014)
    mk-identifiers  BC-04 validators and extractors (pure functions)
    mk-jobs         BC-03 queue and scheduler
    mk-mapping      BC-05
    mk-ai           BC-06 prompts, schema check, evidence check
    mk-graph        BC-07 + BC-08 + BC-09 (rebuilt together, ADR-0005)
    mk-search       BC-10
    mk-egress       BC-11 (the only crate with an HTTP client for project data)
    mk-models       BC-12 catalog, downloads, runtime supervisor, hardware check
    mk-store        SQLite adapters, schema, migrations
    mk-core         application services; wires everything; no Tauri
    mk-bench        BC-13 ground truth and scoring
  apps/
    desktop/        Tauri 2 shell: IPC commands → mk-core; also the --worker entry
    cli/            mangkon-cli: bench, ingest, rebuild, kill-test driver
  tools/
    corpus-gen/     TD-1/TD-3 generator (M0a)
    xtask/          cargo xtask: one-command build, local CI, layer check, notices
  ui/               React + TypeScript (ADR-0003)
```

Rules:
1. Domain crates (`mk-kernel`, `mk-identifiers`, `mk-graph`, `mk-mapping`, the domain parts of others) have **no I/O** and no async runtime. They are plain functions and types.
2. I/O sits behind small traits (ports): `FileSource`, `PdfEngine`, `OcrEngine`, `ModelClient`, `SecretStore`, `Clock`. Adapters live in `mk-store`, `mk-parsers`, `mk-egress`, `mk-models`.
3. Only `apps/desktop` depends on Tauri.
4. Dependencies point inward: apps → `mk-core` → context crates → `mk-kernel`. Never the other way.
5. The worker entry point is a subcommand of the desktop binary. It links `mk-parsers` and `mk-identifiers` only, so there is one binary to ship (NFR-RES-07). (M2c: `mangkon-cli` takes `--worker` too, through the same `mk-core` entry, so the CLI can run Stage A for the kill test and `ingest`.)
6. No `unsafe` code: the workspace sets `unsafe_code = "forbid"`. The one exception is `mk-tesseract-sys`, the FFI to Tesseract's C API. It sets `deny` and allows `unsafe` only in `src/ffi.rs`, behind a safe handle; `mk-parsers` wraps that handle (added in M2a).

## Consequences

**Easier**
- `mangkon-cli bench` runs on every PR in CI (NFR-MNT-05).
- Most logic is unit-testable without mocks for Tauri (NFR-MNT-04).
- Each context can be read and reviewed on its own. Good for an open-source project.

**Harder**
- About 15 crates to keep tidy. Workspace-level lint and version settings are needed.
- Incremental builds are fast, but a clean build compiles more units. Watch NFR-USE-03 (≤ 15 min).

**Follow-ups**
- Add a CI check that `mk-core` and context crates do not depend on `tauri`. **Done in M2a:** `[bans]` in `deny.toml` (`cargo deny check`) and `cargo xtask check-layers`, which walks the whole resolved graph, so a transitive dependency is caught too. Both run in CI; a test PR adding `tauri` to `mk-core` fails both (M2 acceptance criterion 6, checked locally).
- Keep `mk-egress` and `mk-models` the only crates that may use an HTTP client (ADR-0009). **Done in M2a**, by the same two checks (`ureq` added to `mk-kernel` fails both). Both checks cover the desktop targets only; Tauri pulls an HTTP client on iOS and Android, which are not built.
- `mk-worker` added in M2c (ADR-0007). It depends on `mk-kernel` (with its `serde` feature, for the protocol), `mk-parsers` and `mk-identifiers`; `mk-core` depends on it for the supervisor and the `--worker` entry. `cargo xtask check-layers` and `cargo deny check` pass with it.

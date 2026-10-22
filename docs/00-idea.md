# 00 — Idea: Mangkon AI

> Name: **Mangkon AI** · Repo: `mangkon-ai`
> Type: Open-source, local-first desktop app
> Status: **Superseded by `01-brief.md`.** Kept for history; where they disagree, the brief wins. Nothing is built yet.
> Purpose of this doc: input for the PM interview. The interview produces `docs/01-brief.md`.

---

## How to read this doc

This is a **draft full of assumptions**, not final decisions. Many parts are early brainstorming, not confirmed by the owner.

- Items marked **[ASSUMPTION]** must be confirmed or changed in the PM interview.
- Section 17 lists the most important open questions.
- If this doc and `01-brief.md` disagree, **the brief wins**.

---

## 1. One-line summary

A free, open-source, local-first desktop app. You drop in a folder of mixed files. The app finds the people, companies, identifiers, and events in them, and shows **how they connect across different sources**. Every fact links back to its source.

Think "personal mini-Palantir" for a small set of private documents, running on your own machine.

---

## 2. The problem

- People who research or investigate have messy data: PDFs, scans, emails, spreadsheets, notes, web pages.
- The important links are hidden **between** files. Example: the same phone number appears in a scanned invoice, an email, and row 214 of a spreadsheet.
- Current options are weak for this:
  - Chat-with-PDF tools (NotebookLM, AnythingLLM, etc.) summarize files but do not build cross-source links. Many are cloud-based.
  - Free investigator tools (ICIJ Datashare, OCCRP Aleph) are strong on search, weaker on linking and AI. They are heavy to set up.
  - Pro tools (Maltego, Palantir) are expensive, complex, or built for live OSINT and enterprises.
  - Graph-RAG libraries (GraphRAG, LightRAG) are code, not a product.
- Many users **cannot** upload their documents to cloud AI (legal, journalism, privacy).

---

## 3. Target users [ASSUMPTION]

Primary:
- Investigative journalists and researchers
- Due-diligence and background researchers
- Small law firms / paralegals doing document review
- NGOs and human-rights researchers

Secondary:
- Privacy-minded technical users with a lot of personal data

What they have in common: mixed document sets (tens to a few thousand files), a need for privacy, and a need to trust every claim (sources).

**Note for the interview:** the first release is **build-from-source only** (see section 10). So the real first users are **technical users**: developers, OSINT hobbyists, tech-savvy researchers. Non-technical users (lawyers, most journalists) come later, when ready-made builds exist. Decide who the first release is really for.

---

## 4. Core value and "wow" moment

**Killer demo:**
> Drop a mixed folder (PDFs, scans, emails, CSVs) → within minutes see:
> "These 12 people and 5 companies appear in more than one source. Here is how they connect, with a timeline and the exact source for each link."

The main value is **cross-source connections**, not summaries.

**Honest limit:** the app can only connect documents that share something (a person, company, email, phone, address, account, domain, date). If nothing is shared, there is no real link, and the app must not invent one.

---

## 5. Product principles

1. **Local-first.** All data stays on the machine by default. Works fully offline after model download.
2. **Every fact has a source.** No claim without evidence (file + page/row/line + text span).
3. **The user decides.** The app proposes merges and links. The user confirms. User decisions are never overwritten by re-runs.
4. **Zero-config models.** Start the app → it detects hardware → downloads a fitting model → ready.
5. **Honest AI.** Show confidence. Say "no evidence found" instead of guessing.
6. **Small and focused.** One strong workflow beats many weak features.

---

## 6. Scope

### 6.1 In scope for v1 [ASSUMPTION]

- Desktop app (macOS, Windows, Linux), built from source
- Ingest: PDF (text + scanned via OCR), DOCX, TXT, Markdown, CSV, JSON, images (OCR), EML/MBOX email exports, HTML files
- Hard identifier extraction (emails, phones, URLs/domains, IBANs, other patterns)
- Structured data mapping (CSV/JSON columns → entities, without LLM)
- LLM extraction of entities, relationships, and dated events
- Entity resolution with a review queue (accept / reject / edit)
- Entity pages with "appears in these sources"
- Cross-source connections view (the main screen)
- Timeline view
- Simple graph view (neighborhood of an entity, not the full graph)
- Keyword + semantic search
- Q&A over the data, with citations
- Model manager: hardware detection, auto-download from Hugging Face, or use any OpenAI-compatible endpoint
- Export: Markdown report, CSV, GraphML

### 6.2 Out of scope for v1 (later)

- Ready-made installers / signed binaries
- Web / self-hosted version, multi-user
- Live connectors (Notion, Google Drive, IMAP, RSS, scheduled sync)
- Agentic "deep research" mode
- Contradiction detection
- Custom ontology editor (v1 uses a fixed default schema)
- Investigation boards (pin entities + notes)
- Encryption at rest
- Hosted private inference service
- Mobile companion app

---

## 7. Functional requirements (v1)

### FR-1 Projects
- User can create multiple projects (investigations). Each project has its own data.
- User can add files or whole folders (drag & drop or picker).
- User can re-scan a folder. Only new or changed files are processed (content hash).

### FR-2 Ingestion and parsing
- Every file is turned into one common shape: **Document → Segments**.
- Each Segment keeps a **locator**: page + position (PDF/image), line (text), row/column (CSV), message + header/body (email), JSON path (JSON).
- Scanned PDFs and images go through OCR automatically when no text layer exists.
- Failed files are shown with a clear reason. One bad file never stops the batch.

### FR-3 Hard identifiers (no LLM)
- Regex + validation for: email, phone (normalized to E.164 where possible), URL, domain, IBAN, card-like numbers (flag only), crypto addresses, generic IDs (configurable patterns).
- Identifiers are normalized. Same normalized value = same identifier node. This is the strongest cross-source link.

### FR-4 Structured data
- On CSV/JSON import, the user maps columns to entity types in a simple dialog (the app suggests a mapping).
- Rows create entities and relations directly. Evidence = file + row + column.

### FR-5 LLM extraction
- Runs on text segments in chunks.
- Extracts: entities (typed), relations (typed, with direction), events (with date if present).
- Output is forced to a JSON schema (grammar-constrained decoding).
- Every result stores evidence (segment + text span), confidence, extractor type, model id, pipeline version.

### FR-6 Entity resolution
- The app proposes merges using: shared identifiers, fuzzy name match, similar context (embeddings).
- Proposals go to a **review queue** sorted by confidence.
- User can accept, reject, or edit (rename, split, change type).
- User decisions are stored and respected on every re-run.
- Optional setting: auto-accept proposals above a high confidence threshold.

### FR-7 Cross-source connections (main screen)
- List of entities that appear in **2+ different sources**, ranked by number of sources and link strength.
- For each: which sources, which other entities it connects to, evidence snippets.
- Filters: entity type, source type, date range, confidence.

### FR-8 Entity page
- Name, type, aliases, identifiers
- All facts and relations with evidence
- "Appears in" list of sources
- Mini timeline
- Neighborhood graph (1–2 hops)
- Click any evidence → open the source viewer at the exact location

### FR-9 Source viewer
- Shows the original file (PDF page, email, CSV row, text) with the evidence highlighted.

### FR-10 Timeline
- All dated events across the project, filterable by entity and type.

### FR-11 Graph view
- Neighborhood-based (start from an entity, expand step by step).
- No "show everything" hairball by default.

### FR-12 Search
- Keyword (full-text) and semantic search over segments.
- Results link to the source viewer.

### FR-13 Q&A with citations
- User asks in natural language.
- Retrieval = full-text + vector search + graph neighborhood of mentioned entities.
- Answer must cite sources. If there is no evidence, the answer says so.

### FR-14 Model manager
- On first run: detect RAM, VRAM, GPU type, Apple Silicon unified memory, free disk space.
- Show 1–3 recommended model setups (Light / Medium / Heavy) with size, expected speed, license.
- One-click download from Hugging Face: resumable, checksum-verified, disk space checked first.
- Alternative: connect any OpenAI-compatible endpoint (Ollama, LM Studio, vLLM, llama.cpp server, cloud API).
- Clear visual warning when a **remote** endpoint is active ("data leaves this machine").
- Separate slots: extraction model, answer model, embedding model.

### FR-15 Background jobs
- All heavy work runs in a persistent job queue.
- Progress, pause, resume, cancel. Survives app restart.
- User can keep browsing while processing runs.

### FR-16 Export
- Markdown report for an entity or a set of entities (with citations).
- CSV (entities, relations, evidence).
- GraphML.

---

## 8. Non-functional requirements [ASSUMPTION — numbers to confirm]

- **Privacy:** no telemetry, no analytics, no crash reporting. Network is used only for model downloads and user-configured endpoints.
- **Offline:** fully usable offline after models are downloaded.
- **Performance targets (to validate in M0):**
  - 100 mixed files processed to a first useful result in under ~15 minutes on a mid-range machine (16 GB RAM, no big GPU).
  - Identifier + structured results appear within seconds (before LLM extraction finishes).
- **Scale for v1:** up to ~5,000 files / ~1M segments per project.
- **Reliability:** resumable jobs, no data loss on crash, idempotent re-runs.
- **Traceability:** every entity, relation, and event has at least one evidence row.
- **Build from source:** one documented command per OS. Few prerequisites (Rust, Node, and OS build tools). No Docker. No Python needed.
- **UX:** keyboard-friendly, light/dark theme, clear empty and error states.

---

## 9. Architecture [ASSUMPTION — confirm after spikes]

### 9.1 Overview

```
┌────────────────────────────────────────────────────┐
│ Frontend (React + TypeScript, inside Tauri webview) │
│  Connections · Entity pages · Timeline · Graph ·    │
│  Search · Q&A · Review queue · Model manager        │
└───────────────▲────────────────────────────────────┘
                │ Tauri commands + events
┌───────────────┴────────────────────────────────────┐
│ Core (Rust)                                         │
│  Project manager · Job queue · Ingestion pipeline · │
│  Identifier engine · Resolution engine ·            │
│  Retrieval (FTS + vector + graph) · Q&A orchestrator│
│  Model manager (HW detect, HF download)             │
└───────▲──────────────────────▲─────────────────────┘
        │                      │ OpenAI-compatible HTTP
┌───────┴────────┐    ┌────────┴──────────────────────┐
│ SQLite per     │    │ llama.cpp `llama-server`       │
│ project:       │    │ (sidecar, fetched or built     │
│  FTS5 +        │    │  by the build script)          │
│  sqlite-vec +  │    │  or any external endpoint      │
│  graph tables  │    │  (Ollama, LM Studio, cloud…)   │
└────────────────┘    └───────────────────────────────┘
```

### 9.2 Key technical choices (proposed)

| Area | Choice | Why |
|---|---|---|
| Shell | Tauri 2 | Small, native, cross-platform, Rust core |
| Frontend | React + TypeScript + Tailwind | Known stack, rich libraries |
| Core | Rust | Performance, single binary, safe concurrency |
| Storage | SQLite (one DB per project) | Zero setup, portable, easy backup |
| Full-text | SQLite FTS5 | Built in |
| Vectors | sqlite-vec | Keeps everything in one file |
| Graph | Plain tables (entities, relations) + recursive SQL | No extra DB. v1 graph sizes are small enough |
| LLM runtime | llama.cpp `llama-server` as a sidecar | OpenAI-compatible API, GGUF, CPU/GPU/Metal, JSON-schema output |
| Graph UI | Sigma.js or Cytoscape.js | Decide in M0 |
| Timeline UI | vis-timeline or custom | Decide in M6 |
| PDF parsing | pdfium (via Rust bindings) | Good text + positions |
| OCR | Tesseract or a Rust-native OCR (e.g. `ocrs`) | Decide in M0 by quality and build complexity |
| Email | Rust mail parser crate | EML/MBOX |
| Classic NER (optional) | GLiNER via ONNX runtime | Fast entity spotting, reduces LLM load |

**Open question:** Rust-native parsing vs. a Python sidecar (richer libraries like Docling, but harder to build and run). Default: Rust-native.

### 9.3 Processing pipeline

```
File → Hash check → Parse → Segments (with locators)
     → Stage A (fast, no LLM):
         identifiers (regex) · structured mapping (CSV/JSON) · FTS index
         → results visible to the user right away
     → Stage B (slower):
         embeddings · optional NER · LLM extraction (JSON schema)
     → Stage C:
         resolution proposals → review queue → graph update
```

- Each stage writes its results with a `pipeline_version` and `model_id`.
- Changing the model or pipeline version marks affected items for re-processing, but never deletes user decisions.

### 9.4 Data model (first draft)

- `project` — id, name, settings
- `source` — id, path, type, content_hash, status, error
- `segment` — id, source_id, text, locator (JSON), order
- `entity` — id, type, canonical_name, created_by (system|user)
- `alias` — entity_id, name
- `identifier` — id, kind (email|phone|iban|url|domain|crypto|custom), normalized_value
- `entity_identifier` — entity_id, identifier_id
- `relation` — id, from_entity, to_entity, type, confidence
- `event` — id, type, date (with precision), description
- `event_participant` — event_id, entity_id, role
- `evidence` — id, target_type (entity|relation|event|identifier), target_id, segment_id, span_start, span_end, extractor (regex|structured|ner|llm), model_id, confidence, pipeline_version
- `merge_proposal` — id, entity_a, entity_b, reason, score, status
- `user_decision` — id, kind (merge|reject|split|rename|retype), payload, created_at
- `embedding` — segment_id, vector (sqlite-vec)
- `job` — id, kind, status, progress, payload, error

### 9.5 Default schema (fixed in v1)

Entity types: Person, Organization, Location, Event, Money, Date, Project/Topic, Document, Identifier (email, phone, URL/domain, account/IBAN, crypto address, other ID).

Relation types: works_for, member_of, owns, controls, located_in, communicated_with, paid / received_from, participated_in, family_of, associated_with (generic fallback), same_as (from resolution).

### 9.6 Model strategy

- **Rule:** a bigger model at Q4 is usually better than a smaller model at higher precision. Default **Q4_K_M**. Avoid below Q3.
- **Two roles:**
  - Extraction model: fast. Runs on thousands of chunks.
  - Answer model: stronger. Used only for Q&A and summaries.
  - On weak machines, one model does both.
- **MoE models** (many parameters, few active) are a strong default: good quality, fast speed.
- **Embedding model:** small GGUF embedding model, served by the same runtime.
- **Model catalog:** a JSON file in the repo (name, HF repo, file, size, min RAM/VRAM, license, role). Updated without code changes.
- **Hardware tiers (starting point, validate in M0):**
  - Light: 8–16 GB RAM, no dedicated GPU → small model (~4–8B) or small MoE
  - Medium: 16–32 GB RAM or 8–16 GB VRAM → mid MoE or ~14B
  - Heavy: 32 GB+ RAM / 24 GB+ VRAM / Apple Silicon 36 GB+ → larger MoE or ~30B+ dense
- **Quality helpers:** grammar-constrained JSON output, short focused prompts per task, few-shot examples, regex/structured data first so the LLM does less.

### 9.7 Security

- Data lives in the OS app-data folder, one folder per project.
- No telemetry.
- Remote endpoint = clear banner and per-project setting.
- Model files are verified by checksum.
- Encryption at rest (SQLCipher) is planned after v1.

---

## 10. Distribution

### v1: source code only [ASSUMPTION]
- The repo is the product. Users clone and build.
- README gives one clear build command per OS (macOS, Windows, Linux) and lists prerequisites.
- A build script fetches or builds `llama-server` automatically.
- CI (GitHub Actions) builds on all 3 OSes to prove the code compiles. Build artifacts are not published as official releases.
- **No Apple developer account, code signing, or notarization needed** for this stage. A locally built app runs on the builder's own Mac without Gatekeeper problems.

### Trade-off
- Build-from-source limits v1 to technical users. That is fine for an early OSS release, but it narrows the audience (see section 3).

### Later (optional)
- Unsigned binaries on GitHub Releases, with clear "how to open" instructions.
- Package managers: Homebrew, winget, Linux AppImage / Flatpak.
- Signed and notarized builds when there is demand (needs a paid Apple developer account; Windows signing is optional).

---

## 11. Competitors and positioning

| Tool | Strength | Gap we fill |
|---|---|---|
| ICIJ Datashare | Free, local, search, basic entities | Weak cross-source linking, no LLM relations, heavy setup |
| OCCRP Aleph | Strong for large investigations | Server-based, complex to self-host |
| Maltego | Powerful link analysis | Paid, OSINT-focused, not for private document sets |
| NotebookLM | Great summaries | Cloud only, no graph, no cross-source identifiers |
| AnythingLLM / Khoj / Reor | Local chat with docs | Chat-first, no entity graph or resolution workflow |
| GraphRAG / LightRAG | Graph + LLM techniques | Libraries, not a product. No UI, no review workflow |
| Palantir | Full ontology + integration | Enterprise only, huge cost, needs engineers |

**Position:** the easiest free, local tool to find **hidden connections across mixed private documents**, with sources you can trust.

---

## 12. Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Local models extract poorly | Core value fails | Regex + structured first, JSON schema, bigger quantized/MoE models, benchmark in M0 |
| Processing is too slow on laptops | Users give up | Stage A results appear fast, background jobs, small extraction model, process only new files |
| False links | Trust is lost | Evidence on everything, confidence scores, review queue, "no evidence" answers |
| Graph becomes a hairball | Graph view is useless | Neighborhood-only graph, connections list as the main screen |
| Parsing quality (scans, messy PDFs) | Missing data | Test corpus in M0, clear error states, OCR fallback |
| Hard build from source | Few people try it | One-command build per OS, CI on all 3 OSes, good README, demo video |
| Scope creep | Never ships | Strict v1 list (section 6), everything else goes to backlog |
| Model licenses | Legal issues | Show license in catalog, prefer permissive models |
| Big downloads | Bad first run | Disk check, resumable downloads, small default model |

---

## 13. Development constraints

Reference machines and test scope are set in the NFR document.

---

## 14. Milestones [ASSUMPTION — estimates]

Estimates will be updated after M0.

### M0 — Spikes and test corpus (1–2 weeks)
- Build a **test corpus**: ~50–100 mixed files (PDF, scans, emails, CSV) with **known ground-truth links** (who connects to whom, via which file). This is the benchmark for the whole project.
- Spike: PDF/OCR parsing quality and build complexity.
- Spike: extraction quality and speed with 3 model sizes using JSON-schema output.
- Spike: llama-server sidecar in Tauri (Linux + macOS).
- Spike: sqlite-vec + FTS5 in Rust.
- Output: final tech choices, hardware tiers, first model catalog.

### M1 — App shell and model manager (1–2 weeks)
- Tauri app, project create/open, settings.
- Hardware detection, model catalog, HF download (resumable, checksum), start/stop llama-server.
- External OpenAI-compatible endpoint support with remote warning.
- Build scripts + CI for 3 OSes.

### M2 — Ingestion and storage (2 weeks)
- Parsers for all v1 formats → Documents + Segments with locators.
- Content hashing, incremental re-scan, error reporting.
- Job queue (persistent, pause/resume/cancel).
- Source viewer with highlight by locator.

### M3 — Stage A: fast results (1–2 weeks)
- Identifier engine (regex + normalization).
- CSV/JSON column mapping dialog.
- FTS5 keyword search.
- First version of the **cross-source connections** screen (identifiers only).
- ✅ First demo moment: shared identifiers across files.

### M4 — Stage B: AI extraction (2–3 weeks)
- Chunking, embeddings, semantic search.
- LLM extraction of entities, relations, events with evidence.
- Measure against the test corpus (precision/recall).

### M5 — Entity resolution (2 weeks)
- Merge proposals (identifiers, fuzzy names, embeddings).
- Review queue UI.
- User decisions stored and respected on re-runs.

### M6 — Exploration UI (2–3 weeks)
- Full connections screen (entities + identifiers).
- Entity pages.
- Timeline.
- Neighborhood graph view.

### M7 — Q&A with citations (1–2 weeks)
- Hybrid retrieval (FTS + vector + graph).
- Cited answers, "no evidence" handling.

### M8 — Polish and v0.1 repo release (1–2 weeks)
- Export (Markdown, CSV, GraphML).
- README with build steps per OS, architecture overview, contribution guide.
- Demo video using the test corpus.
- Tag v0.1 on GitHub.

**Total: ~4–5 months** to v0.1.

---

## 15. Success criteria for v0.1 [ASSUMPTION]

- A technical user goes from `git clone` to first cross-source result in **under 30 minutes** (including build and model download on a normal connection).
- On the test corpus: most ground-truth links are found, and false links are rare and easy to reject (exact targets set after M0).
- Every shown fact opens its source in one click.
- Works offline after setup.
- Adoption targets (stars, issues, contributors): to be set in the interview.

---

## 16. After v1 (backlog, ordered by likely value)

1. Pre-built binaries and package managers (see section 10)
2. Investigation boards (pin entities, notes, queries)
3. Encryption at rest
4. Custom entity/relation types
5. Folder watch + simple connectors (IMAP, RSS, Obsidian/Notion export)
6. Contradiction detection
7. Agentic deep research on selected entities
8. Self-hosted web version / small team mode
9. Plugin system for parsers and extractors
10. Mobile companion app that talks to the desktop app over the local network (agent-to-agent). The phone does not run heavy models.

---

## 17. Open questions for the PM interview

1. Who is the **first** user of v0.1, given it is build-from-source only?
2. What is their current workaround, and what does it cost them?
3. Which single workflow must work perfectly in v0.1?
4. Success metrics: stars, active users, issues, contributors, own use — what counts as success or failure?
5. Target date and hard stop date?
6. What happens if behind schedule: cut scope or move the date?
7. License: permissive (MIT/Apache-2.0) or AGPL-3.0 (stops closed hosted forks)?
8. Minimum supported hardware?
9. Is a cloud/remote model allowed at all, or local-only in v1?
10. Which v1 features can be dropped if the M0 spikes show poor model quality?
11. Marketing and launch plan: which channels?
12. Rust-native parsing vs. Python sidecar (can be left to M0).

---

## 18. Next docs (one at a time)

1. **01-brief.md** — from the PM interview on this doc
2. **02-test-corpus.md** — benchmark corpus design and ground-truth format
3. **03-spikes.md** — spike plan and result template for M0
4. **04-architecture.md** — detailed architecture after spikes (modules, crates, IPC commands)
5. **05-data-model.md** — final SQLite schema + migrations
6. **06-model-manager.md** — hardware detection, catalog format, download flow
7. **07-pipeline.md** — ingestion, Stage A/B/C, job queue, re-processing rules
8. **08-resolution.md** — merge logic, scoring, review queue UX
9. **09-ui-spec.md** — screens, navigation, key flows
10. **10-milestone-plan.md** — tasks per milestone

# 01 — Brief: Mangkon AI

> Name: **Mangkon AI** · Repo: `mangkon-ai`
> Type: Open-source, local-first desktop app
> Status: Brief approved after PM interview (2026-09-22). §10 milestone order updated to match `06-roadmap.md`; §11 and §17 doc references updated to the docs that exist (2026-10-22). Nothing is built yet.
> Input: `00-idea.md`. **If this brief and `00-idea.md` disagree, this brief wins.**

---

## 1. One-line summary

A free, open-source, local-first desktop app. You drop in a folder of mixed files. The app finds shared identifiers, people, and companies, and shows **how they connect across different sources**. Every link opens its exact source.

---

## 2. The problem

- Researchers and investigators have messy data: PDFs, scans, emails, spreadsheets, notes.
- The important links are hidden **between** files. Example: the same phone number is in a scanned invoice, an email, and row 214 of a spreadsheet.
- Today's tools don't solve this well:
  - Chat-with-PDF tools summarize, but don't link across sources. Many are cloud-only.
  - Free investigator tools (Datashare, Aleph) are strong on search, weak on linking, heavy to set up.
  - Pro tools (Maltego, Palantir) are expensive or built for other jobs.
  - Graph-RAG libraries are code, not a product.
- Many people **cannot** upload their documents to cloud AI.

---

## 3. Target users

### v0.1 — technical researchers
v0.1 is build-from-source only, so the first users must be technical:
- OSINT hobbyists and researchers
- Data journalists who can code
- Security and fraud researchers (scams, crypto fraud, company research)
- Developers who investigate things

They can build from source and give good feedback on GitHub.

### v0.2+ — non-technical users
Lawyers, paralegals, NGOs, most journalists, due-diligence researchers. They come when ready-made installers exist. They stay in the vision, but v0.1 screens are **not** designed around them.

---

## 4. The one workflow that must be perfect

1. User drops a mixed folder (PDF, scans, emails, CSV).
2. **Within seconds:** shared identifiers across files (email, phone, IBAN, domain, crypto address).
3. **Within minutes:** people and companies that appear in 2+ sources.
4. **One click** on any link → the source opens at the exact spot, highlighted.

The **connections screen** and the **source viewer** must be excellent. Everything else is secondary.

---

## 5. Scope for v0.1

v0.1 is built in **layers**. If we are behind, we cut from the top layer down (3 first).

### Layer 1 — Must ship (no LLM needed)
- Desktop app (macOS, Windows, Linux), built from source
- Projects: create, open, add files/folders, incremental re-scan (content hash)
- Ingestion: PDF (text + scanned via OCR), DOCX, TXT, Markdown, CSV, JSON, images (OCR), EML/MBOX, HTML
- Every file → Documents + Segments with exact locators (page, line, row/column, email part, JSON path)
- Clear error per failed file; one bad file never stops the batch
- Hard identifiers: email, phone (E.164), URL/domain, IBAN, crypto addresses, card-like numbers (flag only), custom patterns
- CSV/JSON column mapping to entities (app suggests the mapping)
- Keyword search (full-text)
- **Connections screen** (identifiers in 2+ sources, ranked, with evidence)
- **Source viewer** with highlight
- Background job queue: progress, pause, resume, cancel, survives restart
- Model manager: hardware detection, model catalog, auto-download from Hugging Face, external endpoint support

### Layer 2 — Should ship
- LLM extraction of **people and organizations**, with evidence and confidence
- Simple entity resolution: merge proposals + review queue (accept / reject / edit)
- User decisions are stored and never overwritten by re-runs
- Connections screen shows entities too, not only identifiers
- Simple entity detail (name, aliases, identifiers, "appears in", evidence)
- Semantic search (embeddings)

### Layer 3 — First to cut
- LLM extraction of **relations** (works_for, owns, paid, etc.)
- LLM extraction of **dated events**

### Moved to v0.2+ (important, added step by step)
- Q&A with citations
- Timeline view
- Graph view (neighborhood)
- Export (Markdown report, CSV, GraphML)
- Ready-made installers / signed builds
- Optional Python parsing sidecar (only if M0 shows Rust parsing is not good enough)
- Everything listed as out of scope in `00-idea.md` section 6.2 stays out

---

## 6. Product principles

1. **Local-first.** Data stays on the machine by default. Works offline after model download.
2. **Every fact has a source.** File + page/row/line + text span.
3. **The user decides.** The app proposes merges and links. The user confirms. Re-runs never overwrite user decisions.
4. **Easy models.** Detect hardware → recommend a model → one-click download.
5. **Honest AI.** Show confidence. Never invent a link. "No evidence found" is a valid result.
6. **Small and focused.** One great workflow beats many weak features.

---

## 7. Models and privacy

- **Default:** local model via `llama-server`, with auto-download from Hugging Face (resumable, checksum-verified, disk space checked).
- **Also allowed:** any OpenAI-compatible endpoint — Ollama, LM Studio, a stronger machine on the local network, or a cloud API. It's the user's choice.
- **Honest destination display:** the app always shows **where data goes**, e.g. "Data sent to 192.168.1.20" or "Data sent to api.openai.com".
- Non-local endpoints are **off by default per project** and must be turned on by the user.
- **No telemetry, no analytics, no crash reporting.** Network is used only for model downloads and user-configured endpoints.

---

## 8. Hardware

### Minimum for running the local model (any one of these)
| Option | Notes |
|---|---|
| GPU with **8 GB VRAM** + **16 GB RAM** | NVIDIA fully supported. AMD/Intel (Vulkan): best effort in v0.1 |
| **Apple Silicon** with **32 GB** unified memory | 24 GB may work but is not supported |

Plus **~30 GB free disk** for models and project data.

The minimum applies to **the machine that runs the model**, not the app itself.

### Below minimum
- The app does **not** offer local model download.
- It shows a clear message: "This machine is too weak for local AI. Connect an external model endpoint."
- Layer 1 features (identifiers, CSV mapping, search, connections, source viewer) still work fully.
- The README states the minimum at the top.

Reason: a slow, bad first run hurts the project's reputation more than a smaller audience.

### OS support
- macOS 13+, Windows 10/11, Ubuntu 22.04+. Other Linux: best effort.

### Testing
- The maintainer tests two paths: a machine at or above the minimum, and a below-minimum machine using an external endpoint on the local network.
- Other platforms: CI builds + community testing in v0.1. The README says honestly what is tested.

Final model tiers and timing numbers are set after M1.

---

## 9. Success metrics

### Quality (we control this)
- On the test corpus: **≥ 80% of known links found** (recall) and **≥ 90% of shown links correct** (precision). Final targets confirmed after M1.
- `git clone` → first cross-source result in **under 30 minutes** (build + model download, normal connection, supported hardware).
- Every shown link opens its source in **one click**.
- Works fully **offline** after setup.


---

## 10. Plan

- **Rule:** if behind, **cut scope** (Layer 3 → Layer 2), don't grow the release. Layer 3 is its own milestone (M8), so it is cut first.
- If M1 shows local models extract poorly: v0.1 = Layer 1 (identifier-based connections) + AI extraction marked "experimental".
- **Order:** the riskiest unknown is proven first. Goal, scope, acceptance criteria and spikes for each milestone are in `06-roadmap.md`.

### Milestones (estimates)
| # | Milestone | Weeks |
|---|---|---|
| M0 | Evidence chain spike: test corpus + ground truth, PDF/OCR engine bake-off, exact locators and highlight check, build probe | 1.5 |
| M1 | Local model check: extraction quality and speed on 3 sizes, embeddings with binary vectors, llama-server start/stop, external endpoints | 0.5 |
| M2 | Walking skeleton: parser workers, storage, job queue, thin UI, CI on 3 OSes (TXT + CSV only) | 1.5 |
| M3 | Ingestion for all file types, job queue screen, source viewer | 2 |
| M4 | Stage A: identifiers, CSV/JSON mapping, keyword search, connections screen — **first demo** | 1.5 |
| M5 | Model manager + network gate | 1 |
| M6 | LLM extraction (people + orgs), semantic search | 1.5 |
| M7 | Simple entity resolution + review queue | 1.5 |
| M8 | Relations and events (Layer 3), only if time allows | (1) |
| M9 | Polish, README, Responsible use, demo video, tag v0.1 | 1 |
| | **Total** | **~12** without M8 |

The total is ~1 week more than the first estimate (~11) because of parser worker isolation (ADR-0001). Estimates are updated after M1.

---

## 11. Test corpus and demo data

- **Synthetic core (~80 files):** a made-up investigation (e.g. a fake shell-company scheme). Fake people, companies, emails, phones, IBANs, crypto wallets. Spread across PDF invoices, scanned letters (images), EML emails, a bank CSV, and notes.
- The story is written **first**, so every true link is known (**ground truth**).
- **Real public slice (~20 files):** for messy real-world formats, e.g. public Enron emails, public company registry PDFs, public court filings.
- Published openly (in the repo or a separate `mangkon-demo-corpus` repo), so anyone can run the demo and check the quality numbers.
- Used for: quality benchmark, regression tests, demo video, README GIFs.

Details go in `07-test-corpus.md`, written in M0a.

---

## 12. License

**AGPL-3.0.**
- Stops closed hosted forks: anyone who runs a changed version as a service must share the code.
- No effect on users who run the app locally.
- Trade-off accepted: some companies avoid AGPL code.

---

## 13. Responsible use

- **By design, v0.1 only works on files the user already has.** No web scraping, no people-search APIs, no social media lookups.
- **No fake technical blocking.** It's local open-source software; any block could be removed in a fork.
- **README "Responsible use" section:** what the tool is for (investigation, due diligence, research), the user is responsible for legal use of their data, and privacy laws (e.g. GDPR) still apply.
- **Demos and docs use only synthetic or public data.** Never real private people.

---

## 14. Technical decisions

### Confirmed (can change only with a strong M0 or M1 result)
- Tauri 2 shell, React + TypeScript + Tailwind frontend, Rust core
- SQLite per project, FTS5 for keyword search, sqlite-vec for vectors, plain tables for the graph
- `llama-server` (llama.cpp) as a sidecar, or any OpenAI-compatible endpoint
- JSON-schema (grammar-constrained) output for LLM extraction
- **Rust-native parsing by default.** No Python required to build or run.
- No Docker. One documented build command per OS.

### Decided in M0 and M1
- M0: OCR engine: Tesseract vs `ocrs` (quality + build complexity). Decided in M0c: Tesseract, see ADR-0014.
- M0: Whether Rust parsing is good enough (if clearly not: optional Python sidecar in v0.2, never required)
- M1: Model catalog and hardware tiers (real timing on the test corpus)
- M1: Final precision/recall targets

---

## 15. Top risks

| Risk | Mitigation |
|---|---|
| Local models extract poorly | Layer 1 works without LLM; M1 benchmark; AI marked "experimental" if needed |
| Slow processing | Hardware minimum; Stage A results first; background jobs; only new files re-processed |
| False links destroy trust | Evidence on everything, confidence, review queue, never invent links |
| Hard build from source | One command per OS, CI on 3 OSes, clear README, hardware check |
| Scope creep | Layered scope with a clear cut order, v0.2+ list |
| Misuse (stalking/doxxing) | Files-only design, no OSINT connectors, Responsible use section |
| Limited test hardware | CI + community testing; honest README about what's tested |

---

## 16. Changes from `00-idea.md`

- First users: technical researchers only; others in v0.2+.
- Q&A, timeline, graph view, and export moved from v1 to **v0.2+**.
- Scope is now layered with a clear cut order.
- Milestones shortened to ~11 weeks (was 4–5 months).
- Hardware minimum raised: 8 GB VRAM + 16 GB RAM, or Apple Silicon 32 GB. No local model on weaker machines.
- Remote endpoints: allowed, off by default per project, app shows where data goes.
- License decided: AGPL-3.0.
- Added: test corpus source, Responsible use.

---

## 17. Follow-up docs

The planned list of follow-up docs was replaced by the set that was written (2026-10-22):

| Planned topic | Where it lives now |
|---|---|
| Corpus story, file list, ground-truth format | `07-test-corpus.md`, written in M0a |
| M0 spike plan and result template | `06-roadmap.md` (spikes per milestone) |
| Modules, crates, IPC commands | `05-architecture.md` and `adr/` |
| Data model | `04-domain.md`; the SQLite schema and migrations come with the M2b code |
| Model manager, pipeline, resolution | `04-domain.md`, `05-architecture.md`, ADR-0006/0008/0009; details per milestone (M2, M5, M7) |
| UI spec | Stories in `02-requirements.md`; screens are designed in the milestone that builds them |
| Milestone plan | `06-roadmap.md` |

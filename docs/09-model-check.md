# 09 — Model check: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M1 (spike S1), ADR-0015, ADR-0010, `03-nfr.md` (NFR-PERF-13/24/25, NFR-QA-01/02/08/09/11, NFR-MNT-09), `04-domain.md` (INV-AI-01/03/05/06, INV-MM-04), `07-test-corpus.md`. **If this file disagrees with them, they win.**
> Status: Measured in M1 on RM-1 (2026-10-24). Linux only; macOS and Windows are not verified here. The decisions it fed (Layer 2, A3, A6, R2, brief §14, tiers) were made on 2026-10-24, as proposed in §10.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Question

Spike S1 retires unknown K2 (roadmap §1.1):

- Can a local model that fits the minimum hardware (8 GB VRAM + 16 GB RAM, brief §8) carry Layer 2, the people and organization extraction? Which bigger models are worth listing for stronger machines? How fast are they?
- Do the candidate embedding models keep their quality in a binary (1 bit per dimension) index with float rescoring (ADR-0010, risk R2, gap A6)?
- Does the ADR-0015 runtime path work: find the user-installed `llama-server`, check its build, start it on loopback with a key, health-check, kill and restart it?

Pass marks: people/org precision ≥ 85% and recall ≥ 70% (NFR-QA-08/09); ≥ 2 segments/s on RM-1 (NFR-PERF-24); recall@20 ≥ 0.9 for binary top 200 → float rescore → top 20 (ADR-0010). The external path (Ollama, LM Studio, RM-2 over the LAN) moved to M5 (decided 2026-10-24).

---

## 2. Machine, build and commands

Machine: **RM-1**: Linux x86_64, Ubuntu-based, 24 threads, 93 GB RAM, NVIDIA RTX 5070 Laptop GPU with 8 GB VRAM. `llama-server` build **10863** (`version: 0.4.0-dev (build 10863, commit 88ada91c1)`), a CUDA source build. Measured 2026-10-24 04:11–05:15 UTC on an idle machine (confirmed before the timed runs), one run per model, release build of the harness.

> **M6c:** the harness below was removed; `mangkon-cli llm-bench` replaces it (`16-entities-acceptance.md` §3). The last commit holding it, with the relations probe M8 may want, is `6ff56dd`. The queries are now `crates/mk-bench/data/queries.json`. The raw run outputs named below were never committed and have been deleted; the tables here are what remains of them.

Harness: `spikes/model-check`, a throwaway crate in the `spikes/` workspace (not the main workspace; `ureq` for loopback HTTP lives only there, ADR-0009 covers the main workspace). Public-slice text comes from `pdf-bakeoff dump-text` (hayro + the M0b layout).

```sh
# needs testdata/ (07-test-corpus.md §9) and llama-server ≥ 10863 (install-llama-cpp.md)
cargo build --release --manifest-path spikes/Cargo.toml -p model-check -p pdf-bakeoff --features pdf-bakeoff/hayro
MC=spikes/target/release/model-check
LS=~/AI/llama.cpp/build/bin/llama-server            # stands for the settings path; without it, PATH is searched
M=~/AI/models
$MC runtime  --llama-server $LS -m $M/Qwen_Qwen3.5-4B-Q4_K_M.gguf
$MC segments                                       # segment and gold counts (§3.1)
$MC extract  --llama-server $LS --model $M/Qwen_Qwen3.5-4B-Q4_K_M.gguf [--np 4]
$MC extract  --llama-server $LS --model $M/Qwen_Qwen3.6-35B-A3B-Q8_0.gguf
$MC extract  --llama-server $LS --model $M/Qwen3.8-Flash-Next-UD-IQ3_XXS/Qwen3.8-Flash-Next-UD-IQ3_XXS-00001-of-00003.gguf --label Qwen3.8-Flash-Next-UD-IQ3_XXS
$MC score    --label <run>                         # rescore saved outputs without the model
spikes/target/release/pdf-bakeoff dump-text --engine hayro --dir testdata/td1/public --out spikes/out/public-text.jsonl
$MC embed    --llama-server $LS --model $M/<embedding gguf> [--reuse]
$MC relations --llama-server $LS --model <gguf>
$MC report                                         # tables below → spikes/model-check/out/report.md
```

Outputs go to `spikes/model-check/out/` (ignored): the server log, raw model outputs as JSON Lines (`extract-<run>.jsonl`), run metadata with memory peaks (`extract-<run>.json`), scores, vectors.

---

## 3. Method

### 3.1 Extraction segments and gold

The synthetic slice only: public files have no name annotation (`07-test-corpus.md` §8).

| Source | Segments | What one segment is |
|---|---|---|
| Text PDF | 60 | One page of `page-text.jsonl` (pages are ≤ 1,421 chars). The 500-page ledger gives only its 9 pages with gold mentions plus 20 seeded filler pages (seed 1) as negatives. Attachment PDFs count as their own documents. |
| Scan | 22 | One image or scanned PDF page, as the reference text. OCR noise is not measured here. **ASSUMPTION:** that effect is checked in M6 on real Stage A output. |
| OCR layer | 2 | One page. |
| Notes | 7 | A TXT/MD file decoded by its ground-truth `encoding` (`encoding_rs`), split at line ends at ≤ 2,000 chars (every note fits in one segment). |
| Email | 20 | One EML or MBOX message via `mail-parser`: `From`/`To`/`Cc`/`Subject` lines plus the text body (HTML-only bodies as text, tags stripped). |

Left out: CSV and JSON (names there come through column mapping, US-05.3, Layer 1), DOCX and HTML (29 name mentions). **Covered: 396 of 467 synthetic name mentions (84.8%)**, 111 segments, 331 gold keys. Every covered mention's text was found in its segment (a check in the harness), so no gold mention is invisible to the model.

**Gold per segment:** the set of `name_key`s (`mk_bench::keys::name_key`, the US-11.1 normalization the ground truth uses: lower case, punctuation to space, legal suffixes dropped for organizations) of the name mentions in that segment.

### 3.2 Prompt and request

One prompt (version 1) for every model. It asks for every person and organization named in the text, with `name` exactly as written, `type` (`person`/`org`), a short exact `evidence` quote and a `confidence` (US-10.1 AC1). It excludes job titles alone, places, products, email addresses, domains and identifiers, and asks for compact JSON. The full text is `SYSTEM_PROMPT` in `spikes/model-check/src/extract.rs`. It was settled on an 8-segment smoke sample before the timed runs (two changes: compact JSON; reference numbers excluded); it was not tuned on the results.

Request: `/v1/chat/completions` with `response_format` = JSON schema (strict, grammar-constrained, brief §14), `temperature` 0, `seed` 1, `max_tokens` 2048, `chat_template_kwargs.enable_thinking` false. Server: `-c 8192 -np 1 --cache-ram 0 --reasoning off`, everything else at the llama.cpp defaults (`--fit on` places layers and MoE experts between GPU and CPU). One request at a time (NFR-PERF-25); one extra run on the 4B model with `-np 4` and 4 client threads, as input for gap A3.

**Prediction pipeline:** strict parse (schema shape, no extra fields, confidence in 0..1; on failure one retry that tells the model the error, then logged and skipped, INV-AI-03) → evidence span check (evidence found in the segment after whitespace normalization, else discarded and counted, INV-AI-01) → `name_key(type, name)`.

**Metrics:** micro precision and recall over the per-segment key sets; a wrong type is one false positive and one false negative. Also: "shown" (only confidence ≥ 0.5, INV-AI-05), split by kind and by source, and a type-agnostic variant. Diagnostics: schema failures (first try / after retry), span discards, kept names not in the segment, truncated outputs, tokens per segment, segments/s (wall time of the whole run after a warm-up), peak RSS (`VmHWM`, includes mapped model pages) and peak VRAM (`nvidia-smi` for the server PID, sampled every 500 ms).

**Link estimate (NFR-QA-01/02):** a predicted link is a predicted name key in ≥ 2 documents; it is compared with gold name keys in ≥ 2 documents. Name-key level only (merges are M7), so it is an estimate, not the NFR measure.

### 3.3 Embedding check

- **Corpus:** every page of `page-text.jsonl` (including all 500 ledger pages), the note and email segments, and the 354 public-slice pages (`dump-text`), split into ~500-char windows at whitespace; windows under 40 chars dropped: **3,163 windows**.
- **Queries:** 50 hand-written queries (`spikes/model-check/queries.json`: 30 on the story, 20 on the public files; synthetic and public topics only, NFR-LEG-04) plus 50 seeded known-item queries (seed 2: the longest sentence of a random window, ≤ 200 chars).
- **Per model:** `llama-server --embedding -c 8192 -np 4 -b 2048 -ub 2048`, the model's own query/document prefixes (they become catalog fields), vectors L2-normalized. Exact search: float cosine top 20. Binary: sign bit per dimension, Hamming top k (k = 40, 100, 200, ties by index) → rescore with f32 → top 20; also rescore with f16 vectors (ADR-0010 allows f16 storage) and binary-only top 20. recall@20 = overlap with the exact top 20 / 20, averaged over queries.
- **Caveat:** TD-1 is about 300× smaller than TD-2. A 200-candidate pool is 6.3% of this corpus but 0.02% of TD-2, so this test is easier than production. See §10 (S6 proposal).

### 3.4 Layer 2 decision rule (fixed before measuring)

- **Go:** a model that fits the minimum tier (8 GB VRAM + 16 GB RAM; of the three, only Qwen3.5-4B) meets P ≥ 85% and R ≥ 70%.
- **Experimental:** only bigger-tier models meet the targets, or the minimum-tier model is within 10 points of them. Options then: raise the hardware minimum (brief §8), or ship Layer 2 marked Experimental (US-10.3).
- **Cut:** no model reaches P ≥ 70%. **ASSUMPTION:** the 70% threshold is a proposal.
- A speed miss alone does not cut Layer 2; it goes to gap A3.

---

## 4. Runtime (S1-R)

`model-check runtime` on RM-1, 2026-10-24 04:11 UTC, model Qwen3.5-4B Q4_K_M. Server args: `-c 4096 -np 1`.

| Step (ADR-0015) | Result | Detail |
|---|---|---|
| Find: settings path first | ✅ | `--llama-server` path used. With it left out and the binary's folder on `PATH`: found on `PATH`. With neither: "llama-server not found (see docs/install-llama-cpp.md)" |
| `--version` → build | ✅ | `version: 0.4.0-dev (build 10863, commit 88ada91c1)` → build 10863, commit `88ada91c1` |
| Minimum check | ✅ | 10863 ≥ 10863. Below it: "llama.cpp build `<n>` found; build 10863 or newer is needed (see docs/install-llama-cpp.md)" |
| Start + `/health` | ✅ | `--host 127.0.0.1 --port <free> --api-key <64 hex>`; ready after 3.8 s (cold), 1.3 s (warm) |
| Key enforced | ✅ | No key → 401, wrong key → 401, right key → 200. `/health` needs no key |
| Loopback only | ✅ | The port listens on `127.0.0.1` only (`/proc/net/tcp{,6}`) |
| SIGKILL | ✅ | Process gone, port closed |
| Restart | ✅ | New port, new key, ready after 1.3 s; the old key → 401 |
| Stop | ✅ | Ends on SIGTERM |

Unit tests with fake binaries (`#[cfg(unix)]`) cover both `--version` forms, unreadable output, settings path before `PATH`, a settings path that is gone, a non-executable file on `PATH`, and the refusal text (`adr_0015_*` in `spikes/model-check/src/runtime.rs`).

**Differences from ADR-0015.** The decision holds. Two assumptions in it were wrong, and both are recorded here and in the install guide (ADR-0015 is not edited):
- The `--version` line is `version: <semver> (build <n>, commit <hash>)`, not `version: <build> (<commit>)`. The parser accepts both. It is printed on stderr.
- The minimum is **build 10863** (the build M1 tested), not `b11669`. The first `vX.Y.Z` release at or above it is `v0.4.1` (build 10964); `v0.4.0` is build 10809, below it.
- Not in the ADR, found on the way: `--cache-ram` defaults to 8 GiB of prompt cache in RAM, and `--fit on` (the default) places layers and MoE experts between GPU and CPU by itself. The app should pass `--cache-ram 0` (or a small value) to keep RAM use predictable, and can rely on `--fit` instead of computing `-ngl`. **ASSUMPTION** until M5.
- Several distribution packages are older than build 10863 (Ubuntu 26.04: 8681; Fedora 44: 6153; Nix 26.05: 9190). The install guide says so.

---

## 5. Extraction (S1-X)

Runs 2026-10-24 04:38–05:10 UTC, 111 segments each, prompt v1.

### 5.1 Pass marks

| Model (file) | Fits tier | P (≥ 85%) | R (≥ 70%) | Segments/s (≥ 2) | Full TD-1 Stage B, est. | RSS peak | VRAM peak |
|---|---|---|---|---|---|---|---|
| Qwen3.5-4B Q4_K_M (2.9 GB) | Minimum (8 GB VRAM + 16 GB RAM) | ❌ 82.9% | ✅ 94.0% | ❌ 0.66 | ~24 min | 3.3 GB | 3.3 GiB |
| Qwen3.5-4B Q4_K_M, `-np 4` | Minimum | ❌ 81.7% | ✅ 93.4% | ❌ 1.39 | ~11 min | 3.3 GB | 4.2 GiB |
| Qwen3.6-35B-A3B Q8_0 (36.9 GB) | 8 GB VRAM + 48 GB RAM | ✅ 91.2% | ✅ 96.7% | ❌ 0.17 | ~94 min | 37.5 GB | 6.5 GiB |
| Qwen3.8-Flash-Next UD-IQ3_XXS (82.0 GB, 3 files) | 8 GB VRAM + 64 GB RAM | ✅ 93.4% | ✅ 98.5% | ❌ 0.12 | ~130 min | 50.1 GB | 6.5 GiB |

- **Quality:** both bigger models pass; the minimum-tier model misses precision by 2.1 points and passes recall easily.
- **Speed:** no model reaches 2 segments/s one request at a time. Decode speed (server log): 4B ~93 tokens/s, 35B-A3B ~36, Flash-Next ~19; about 115–155 output tokens per segment. Four slots double the 4B's throughput (1.39/s) at the same quality.
- **Full TD-1 estimate:** a full Stage B pass runs on every page, including the whole ledger and the public slice, about 940 segments (vs the 111 scored here) ÷ the measured rate. **ASSUMPTION:** public pages are denser than the scored ones, so this is a lower bound. NFR-PERF-13 (≤ 20 min, people/orgs + embeddings) is met only by the 4B with 4 slots.
- **RSS** is `VmHWM` and includes mapped model pages. Flash-Next maps 82 GB but touched 50 GB; with less RAM it would page from disk. VRAM stays under 7 GiB for every model because `--fit` keeps MoE experts on the CPU.
- Schema: **0 failures** in 444 requests, no retry needed (grammar-constrained output). Span check discards: 0–4 per run (≤ 1.0%). No output was truncated.

### 5.2 Diagnostics

| Run | Shown (conf ≥ 0.5) P / R | Identifier-like names dropped P / R | Person P / R | Org P / R | Link estimate P / R (gold 38 links) |
|---|---|---|---|---|---|
| Qwen3.5-4B | 82.9% / 94.0% | **92.0% / 93.7%** (38 dropped) | 92.3% / 96.0% | 78.0% / 92.7% | 75.5% / 97.4% |
| Qwen3.5-4B `-np 4` | 81.7% / 93.4% | 90.6% / 93.1% (39) | 92.3% / 96.0% | 76.2% / 91.7% | 72.5% / 97.4% |
| Qwen3.6-35B-A3B | 92.0% / 96.7% | 94.1% / 96.7% (11) | 90.5% / 99.2% | 91.6% / 95.1% | 90.2% / 97.4% |
| Qwen3.8-Flash-Next | 93.4% / 98.5% | 96.2% / 98.5% (10) | 88.6% / 99.2% | 96.7% / 98.1% | 92.5% / 97.4% |

By source, P / R:

| Run | Text PDF | Scan | OCR layer | Notes | Email |
|---|---|---|---|---|---|
| Qwen3.5-4B | 93.0% / 91.7% | 92.3% / 100% | 90.0% / 100% | 82.1% / 86.5% | **61.9%** / 95.6% |
| Qwen3.5-4B `-np 4` | 91.7% / 91.7% | 92.1% / 97.2% | 90.0% / 100% | 82.1% / 86.5% | 60.2% / 95.6% |
| Qwen3.6-35B-A3B | 97.9% / 98.6% | 97.2% / 97.2% | 90.0% / 100% | 89.5% / 91.9% | 75.3% / 94.1% |
| Qwen3.8-Flash-Next | 96.6% / 99.3% | 100% / 97.2% | 100% / 100% | 97.4% / 100% | 79.5% / 97.1% |

Type-agnostic scores equal the typed ones for every run: no person/org confusion that changed a key.

### 5.3 Error analysis

- **Email is the weak source for every model.** Most false positives are identifiers listed as names despite the prompt: domains and addresses (`velmora-port.example` ×7, `d.oh.private@harbourmail.example`), or a name read out of an address (`daniel.okonkwo-hart@…` → "daniel.okonkwo-hart", `Irene@Brightwater-Logistics.example` → "Irene"). Dropping names with no whitespace that hold `@` or `.` (the "identifier-like" column) lifts the 4B from 82.9% to 92.0% precision and costs 1 true positive. In the product, Stage A has already found these identifiers, so an "LLM mention overlaps an identifier mention" rule is the natural M6 version. **This rule was found by looking at these results**, so it is not part of the pass decision (§3.4); M6 must re-measure it.
- The 4B also lists mailbox names ("Brightwater Logistics Accounts"), phone numbers and one Bitcoin address as organizations, and misses short forms ("Brightwater" alone ×6, "VPA" ×3, "Mr Okonkwo-Hart" ×2).
- The bigger models' remaining errors are mostly the same address cases, plus "Port Authority" (the short form, 2 per model). The ground truth does not annotate "Port Authority" as a mention of Velmora Port Authority; that is a gold gap rather than a model error (§11).
- Recall is high for every model (≥ 93%). The misses are mostly one-word short forms.

---

## 6. Embeddings (S1-E)

Runs 2026-10-24 05:11–05:13 UTC. 3,163 windows, 100 queries (50 hand-written + 50 seeded). recall@20 against exact float search:

| Model (file) | Dims | Binary only | Pool 40 | Pool 100 | **Pool 200** (pass ≥ 0.9) | Pool 200, f16 rescore | Worst query (pool 200) | Hand / seeded (pool 200) | Windows/s | RSS / VRAM |
|---|---|---|---|---|---|---|---|---|---|---|
| Qwen3-Embedding-0.6B Q8_0 (0.64 GB) | 1024 | 0.699 | 0.862 | 0.954 | ✅ **0.984** | 0.984 | 0.70 | 0.974 / 0.993 | 65 | 1.7 GB / 2.8 GiB |
| bge-m3 Q8_0 (0.63 GB) | 1024 | 0.642 | 0.822 | 0.935 | ✅ **0.972** | 0.972 | 0.60 | 0.962 / 0.981 | 226 | 1.8 GB / 0.5 GiB |
| nomic-embed-text-v1.5 Q8_0 (0.15 GB) | 768 | 0.566 | 0.755 | 0.903 | ✅ **0.954** | 0.954 | 0.40 | 0.926 / 0.982 | 533 | 0.8 GB / 0.5 GiB |

- **R2 holds on TD-1** for all three models: binary top 200 → f32 rescore → top 20 keeps recall@20 at 0.95–0.98. All three also pass at pool 100. Binary alone is not enough (0.57–0.70), so rescoring is needed, as ADR-0010 says.
- **f16 storage costs nothing:** f16 rescoring gives the same recall to three decimals.
- Single queries can be much worse (0.40–0.70): a few hand-written queries whose exact top 20 is spread over near-equal windows (ledger rows, Federal Register boilerplate).
- Seeded self-hit (the query's own window in the exact top 20) is 0.86–0.94. Misses are near-duplicate windows, mostly ledger pages, which outrank the source.
- Speed: nomic is 8× faster than Qwen3-Embedding; bge-m3 is in between. Qwen3-Embedding is a 28-layer decoder, the other two are encoders. One request of 32 windows at a time, 4 server slots.
- Index size at 1M segments: 128 MB binary for 1024 dims (96 MB for 768), plus 2 GB of f16 vectors in `project.db` for rescoring.
- Prefixes used (catalog fields): Qwen3-Embedding query `Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery:`, no document prefix; nomic `search_query: ` / `search_document: `; bge-m3 none.
- Caveat (§3.3): pool 200 is 6.3% of this corpus but 0.02% of TD-2. Recall at TD-2 scale is not shown here.

---

## 7. Relations probe (S1-P)

Qwen3.8-Flash-Next (the best extractor), 2026-10-24 05:13 UTC. INV-AI-06 relation types, one prompt (`spikes/model-check/src/relations.rs`), the 14 segments that hold both evidence mentions of a gold relation (18 of the 20 gold relations; the other 2 are in left-out formats).

| Gold | Predicted | Correct | Correct, either direction | Span discarded | Names not resolved to the cast | P | R |
|---|---|---|---|---|---|---|---|
| 18 | 27 | 13 | 14 | 3 | 2 | 48.1% | 72.2% |

Informative only. Recall is fair; precision is low, partly because the ground truth may not list every relation a text states (**ASSUMPTION:** the 20 relations are not a complete annotation, `07-test-corpus.md` §4 does not claim one). Run time 100 s for 14 segments. Input for the M8 go/no-go: relations from the biggest local model are usable only as "experimental" and need their own precision work; M8 stays cut first. **M8 was cut on 2026-11-07** (`06-roadmap.md` §4 #14).

---

## 8. NFR inputs

| NFR | Target before M1 | Measured in M1 (RM-1) | Decision (2026-10-24) |
|---|---|---|---|
| NFR-QA-08 people/org precision | ≥ 85% | 4B 82.9%; 35B-A3B 91.2%; Flash-Next 93.4% | Keep 85%. |
| NFR-QA-09 people/org recall | ≥ 70% | 94.0% / 96.7% / 98.5% | Keep 70%. Reference text only; OCR noise (M6) will lower recall on scans. |
| NFR-PERF-24 LLM speed | ≥ 2 segments/s | 4B 0.66 (1 slot), 1.39 (4 slots); 35B-A3B 0.17; Flash-Next 0.12 | ≥ 1 segment/s on RM-1 with the minimum-tier model; reached with 4 slots (gap A3). |
| NFR-PERF-25 parallel requests | 1 request at a time per local model | 4 slots: 2.1× throughput, same quality (P 81.7% vs 82.9%), +0.9 GiB VRAM | Stage B may use up to 4 slots on the local model. |
| NFR-PERF-13 Stage B on TD-1 | ≤ 20 min | Estimate ~940 segments: 4B ~24 min (1 slot), ~11 min (4 slots); 35B-A3B ~94 min; Flash-Next ~130 min. Embeddings ~2–15 s per 1,000 windows | Keep ≤ 20 min, for the recommended minimum-tier model with 4 slots. Bigger models are a quality choice and show an estimate instead. |
| NFR-QA-01 link recall | ≥ 80% | Name-key estimate 97.4% for every model | Keep 80%. Re-measured in M4 (identifiers) and M7 (entities). |
| NFR-QA-02 link precision | ≥ 90% | Name-key estimate 4B 75.5%; 35B-A3B 90.2%; Flash-Next 92.5% | Keep 90%. Re-measured in M4 and M7. The estimate counts only name links, where the 4B's address errors land. |
| NFR-QA-11 invented evidence shown | 0 | Span check discarded 0–4 per run (≤ 1.0%); none of them kept | Confirmed: the span check is needed and cheap. |
| NFR-MNT-09 catalog | Versioned JSON + schema, CI | `crates/mk-models`: `catalog/catalog.json` v0 + `schema/catalog.schema.json`, `cargo test -p mk-models` | Done (CI from M2a). |

---

## 9. Hardware tiers and catalog v0

Catalog v0 is `crates/mk-models/catalog/catalog.json` (schema `crates/mk-models/schema/catalog.schema.json`, checks in `Catalog::validate`): 3 extraction models, 3 embedding models, 2 tiers. File names, sizes and SHA-256 come from the files measured here; each model is pinned to a Hugging Face commit whose file has that SHA-256 (checked against the Hub's LFS hashes on 2026-10-24). The 4B and 35B-A3B files were re-uploaded upstream on 2026-05-19/20 with other bytes; the catalog pins the older commit that holds the measured file.

| Model | Repo @ commit | License | Peak VRAM / RAM | Listed as |
|---|---|---|---|---|
| Qwen3.5-4B Q4_K_M | `bartowski/Qwen_Qwen3.5-4B-GGUF` @ `8a544ca0` | Apache-2.0 | 3.3 GiB / 3.3 GB | Recommended, minimum tier |
| Qwen3.6-35B-A3B Q8_0 | `bartowski/Qwen_Qwen3.6-35B-A3B-GGUF` @ `1558bc0b` | Apache-2.0 | 6.5 GiB / 37.5 GB | Recommended, 48 GB RAM tier |
| Qwen3.8-Flash-Next UD-IQ3_XXS | `unsloth/Qwen3.8-Flash-Next-GGUF` @ `766911a6` | Qwen Community License 1.0 (`LicenseRef-`; allows local use; limits for very large commercial services and model-as-a-service businesses) | 6.5 GiB / 50.1 GB | Listed, not recommended: +2.2 points over 35B-A3B at 1.4× the time |
| bge-m3 Q8_0 | `ggml-org/bge-m3-Q8_0-GGUF` @ `9eba04c5` | MIT | 0.5 GiB / 1.8 GB | Recommended, every tier: multilingual, passes with margin, 3.5× faster than Qwen3-Embedding |
| Qwen3-Embedding-0.6B Q8_0 | `Qwen/Qwen3-Embedding-0.6B-GGUF` @ `370f27d7` | Apache-2.0 | 2.8 GiB / 1.7 GB | Listed: best recall, slowest |
| nomic-embed-text-v1.5 Q8_0 | `nomic-ai/nomic-embed-text-v1.5-GGUF` @ `0188c9bf` | Apache-2.0 | 0.5 GiB / 0.8 GB | Listed: fastest, smallest margin, English-centred |

Tiers (approved 2026-10-24; brief §8):

| Tier | Machine | Extraction | Embeddings | Why |
|---|---|---|---|---|
| `minimum` | 8 GB VRAM + 16 GB RAM, or 32 GB unified memory | Qwen3.5-4B Q4_K_M | bge-m3 | The only measured model that fits; uses 3.3 GiB VRAM + 3.3 GB RAM, leaving room for the app (NFR-RES-02). |
| `large-ram` | 8 GB VRAM + 48 GB RAM, or 64 GB unified memory | Qwen3.6-35B-A3B Q8_0 | bge-m3 | Passes P and R; 37.5 GB RSS + app + OS fit in 48 GB. |

Apple unified-memory thresholds are not measured (no Mac in M1, platform scope Linux first): **ASSUMPTION**, from the RAM + VRAM peaks. Disk: the minimum-tier models take 3.5 GB, well inside the brief's ~30 GB.

---

## 10. Inputs for the M1 decisions

All eight were decided on 2026-10-24 as proposed below. They are recorded in the roadmap (M1 outcome, §1.4, §4), the brief (§8, §10, §14), architecture §10 and the NFR rows.

| # | Decision | Rule or evidence | Decided |
|---|---|---|---|
| 1 | **Layer 2: Go / Experimental / Cut** (roadmap §4 #6) | §3.4 applied: the minimum-tier model passes recall but misses precision by 2.1 points (within 10); both bigger-tier models pass. → **Experimental.** With identifier-like names dropped, the 4B reaches 92.0% (§5.3), but that rule was found on these results. | Ship Layer 2 marked **Experimental** (US-10.3 flag). M6 builds the identifier-overlap rule and re-measures on Stage A output; if the minimum-tier model then passes NFR-QA-08/09, the label is dropped at the M6 exit. Keep the hardware minimum (brief §8). |
| 2 | **A3: Stage B on large projects** (§4 #4) | No model reaches 2 segments/s; 4 slots give 1.39/s. TD-2 (1M segments) would take ~8 days at that rate. | Stage B uses up to 4 slots (NFR-PERF-25). On projects over the TD-1 estimate (≥ 20 min), Stage B runs on documents the user picks, segments with identifiers first, resumable in the background. NFR-PERF-24 becomes ≥ 1 segment/s. |
| 3 | **A6: binary vectors as a catalog rule** (§4 #5) | All three models pass at pool 200 (0.954–0.984). | Adopt: a listed embedding model needs `binary_recall_at_20` ≥ 0.9 (checked by `Catalog::validate`). |
| 4 | **R2 + brief §14** (§4 #2) | ADR-0010's design holds on TD-1; f16 rescoring loses nothing. | Change brief §14 "sqlite-vec for vectors" to "binary vector index with float rescoring (ADR-0010)". ADR-0010 stays Proposed (S4, S6 open). |
| 5 | S6 recall at scale | TD-1 pool share is 300× larger than at TD-2. | Add to S6 (M6): recall@20 of 100 queries at TD-2 scale, same pass mark. |
| 6 | Hardware tiers (brief §8) | §9. | Two tiers as in §9; the brief minimum is unchanged. |
| 7 | M5–M8 re-baseline (roadmap §1.4) | Runtime mechanics proven; external path moved to M5; relations weak (§7). | M5 1 → 1.25 weeks (external endpoints and their structured-output check move in); M6 1.5 (adds the identifier-overlap rule and 4-slot Stage B; meaning search unchanged); M7 1.5; M8 (1), go/no-go leaning no. Total ~12.25 weeks without M8. |
| 8 | Keep `spikes/model-check` past M2a | It is the only LLM benchmark until M6 builds NFR-MNT-06. | Keep it until M6; remove only `pdf-bakeoff` in M2a. |

---

## 11. Open points and conflicts

- **Gold gap: "Port Authority".** The short form of Velmora Port Authority is printed but not annotated, so every model gets ~2 false positives for it. Fix in the generator's alias list (`07-test-corpus.md` §3) when TD-1 is next regenerated.
- **Reference text, not OCR output,** for scans (§3.1). M6 measures extraction on real Stage A output.
- **The prompt was settled on 8 segments of TD-1** (§3.2). It is a small risk of fitting TD-1; M6 should keep the same prompt version and report on a fresh corpus slice if one exists.
- **The identifier-overlap rule** (§5.3) is a post-hoc finding; it changes the Layer 2 outcome only if M6 confirms it.
- **RSS includes mapped model pages.** For the MoE models, `VmHWM` is what the OS must hold to avoid paging from disk. A machine with less RAM would run, slower. Not measured.
- **Speed was measured with a warm page cache** (files read before; load 35 s for the big models). A cold first load from disk is slower; not measured.
- **The external path** (Ollama, LM Studio, RM-2 over the LAN, their `structuredOutput` flags) moved to M5 (decided 2026-10-24).
- **NFR-USE-01's install step** was not timed (`install-llama-cpp.md`): only archive sizes are known.
- **macOS and Windows** are not verified here (platform scope). Apple tier thresholds are an estimate.

# ADR-0010: Search — FTS5 keyword index + binary vector index with rescoring

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-PERF-30/31, NFR-CAP-03, NFR-RES-01, INV-SR-01..04, domain D4, ADR-0004, ADR-0009

## Context

- Keyword search on TD-2 (1M segments): ≤ 200 ms p95 for the first 50 results (NFR-PERF-30).
- Meaning search on TD-2, including the query embedding: ≤ 1 s p95 for the top 20 (NFR-PERF-31).
- Up to 2M segments per project (NFR-CAP-03).
- App memory when idle ≤ 500 MB (NFR-RES-01).
- Meaning search compares only vectors from the same model (INV-SR-02). Keyword search works without embeddings (INV-SR-04).
- Every result points to one segment with a locator (INV-SR-01).
- Size check: 1M segments × 768 dimensions × 4 bytes = **~3 GB** of float vectors. 2M = ~6 GB. Too big to keep in memory or to scan per query.

## Options considered

**Keyword search**

| Option | ➕ | ➖ |
|---|---|---|
| **SQLite FTS5** | Built in. Same transaction as the segments. No extra files. | Ranked search (BM25) scores every match. A very common word over 1M segments may be slow. |
| Tantivy | Fast top-k ranking. Rich queries. | A second store that commits separately. Must be kept in sync after crashes. |

**Meaning search**

| Option | ➕ | ➖ |
|---|---|---|
| Float vectors in SQLite, brute force (for example `sqlite-vec`) | Simple. Exact. | ~3 GB to scan per query at 1M × 768. Too slow and too much I/O. |
| HNSW index (for example `usearch`) | Very fast. | The index holds full vectors + graph in memory (GBs at 1M). Build time. Approximate. |
| LanceDB | Disk-based ANN. | Heavy dependencies (Arrow). Build time and binary size (NFR-USE-03, NFR-RES-07). |
| **Binary-quantized flat index + rescoring** | 1 bit per dimension: ~96 MB per 1M × 768. Exact scan in tens of ms. Rescoring with float vectors fixes most of the quality loss. | Quality depends on the embedding model. Must be checked. |

## Decision

**Keyword search: SQLite FTS5.**
- External-content FTS5 table over `segment.text`. Tokenizer `unicode61` with `remove_diacritics 2`.
- Ranked by BM25, `LIMIT 50`, keyset paging for more.
- Exact identifier lookups do **not** use FTS. They use the identifier tables (much faster and exact).
- Results from Missing documents are shown with a "Missing" badge (domain D4).
- **Fallback:** if M4 (spike S4) shows FTS5 misses 200 ms p95 on TD-2 (risk R1), add Tantivy as a **derived** index under `index/`. It is rebuilt from `project.db` whenever it is missing or behind. The source of truth does not change.

**Meaning search: binary flat index + float rescoring.**
- **Source of truth:** float vectors (f32, or f16 to save space) in `segment_embedding` in `project.db`, keyed by segment and model (INV-SR-02).
- **Derived index:** `index/vectors-<model-id>.bin`. One bit per dimension (the sign). Rows in segment order. Memory-mapped, loaded only when meaning search is used, and released after 10 min idle (NFR-RES-01).
- **Query:**
  1. Embed the query through the egress gate (INV-SR-03, ADR-0009).
  2. Scan the binary index with Hamming distance (popcount), in parallel. Keep the top 200 (10× oversampling, ASSUMPTION).
  3. Load those 200 float vectors from SQLite. Rescore with cosine similarity. Return the top 20.
- **Expected cost** at 1M × 768 (ASSUMPTION, check in M6, spike S6): scan ~20–50 ms, rescore ~10–30 ms, query embedding ~50–300 ms. Well inside 1 s.
- The index is rebuilt after a model switch (ADR-0005) and on demand. If it is missing or stale, meaning search says "Preparing meaning search…" and rebuilds it.
- **Fallback:** if recall@20 against exact search is below 0.9 on TD-1 (risk R2), first try int8 vectors in the index (~4× larger), then an HNSW index (`usearch`).

## Consequences

**Easier**
- No new storage engine in v0.1. Everything is either in `project.db` or rebuildable from it.
- Memory stays small: ~100–200 MB index at full scale, only while meaning search is in use.
- Both fallbacks are additive. They do not change the data model.

**Harder**
- The embedding model must work well with binary vectors. This becomes a model-catalog rule (gap A6).
- FTS5 makes `project.db` bigger (NFR-CAP-08).

**Follow-ups**
- Benchmarks: recall@20 of binary + rescore vs exact on TD-1 in M1 (spike S1); 100 keyword queries on TD-2 in M4 (spike S4); 100 meaning queries on TD-2 in M6 (spike S6).
- **M1 result (2026-10-24, `09-model-check.md` §6):** on 3,163 TD-1 windows and 100 queries, binary top 200 → f32 rescore → top 20 keeps recall@20 at 0.984 (Qwen3-Embedding-0.6B), 0.972 (bge-m3) and 0.954 (nomic-embed-text-v1.5). Pool 100 also passes (0.903–0.954); binary alone does not (0.57–0.70). f16 rescoring gives the same recall, so f16 storage is fine. R2 (quality) is retired for TD-1; the fallbacks are not needed. At 1024 dims the index is 128 MB per 1M segments. Brief §14 now names this design. Status stays Proposed: FTS5 (S4) and latency plus recall at TD-2 scale (S6) are still open.

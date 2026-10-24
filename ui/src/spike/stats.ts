/** Nearest-rank percentile of an ascending array; 0 for an empty one. */
export function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) return 0;
  const rank = Math.ceil((p / 100) * sorted.length) - 1;
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank))] ?? 0;
}

export interface FrameSummary {
  frames: number;
  meanMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  maxMs: number;
  slowFrameShare: number;
  jankFrameShare: number;
}

/** Frame-time summary for NFR-PERF-04: one 60 Hz frame is 16.7 ms; 25 ms marks jank. */
export function summarizeFrames(deltas: readonly number[]): FrameSummary {
  const sorted = [...deltas].sort((a, b) => a - b);
  const n = sorted.length;
  const share = (limit: number) => (n === 0 ? 0 : sorted.filter((d) => d > limit).length / n);
  return {
    frames: n,
    meanMs: n === 0 ? 0 : sorted.reduce((a, b) => a + b, 0) / n,
    p50Ms: percentile(sorted, 50),
    p95Ms: percentile(sorted, 95),
    p99Ms: percentile(sorted, 99),
    maxMs: sorted[n - 1] ?? 0,
    slowFrameShare: share(16.7),
    jankFrameShare: share(25),
  };
}

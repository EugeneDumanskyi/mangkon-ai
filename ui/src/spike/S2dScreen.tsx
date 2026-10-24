// Spike S2d (K7, NFR-PERF-04): 100,000 virtualized rows over keyset pages, and a scripted scroll
// that records frame times. Throwaway: replaced by the real connections list in M2d. Results are
// in docs/10-walking-skeleton.md §S2d.
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { commands } from "../ipc/commands";
import type { FrameStats } from "../ipc/bindings/FrameStats";
import { PAGE_SIZE } from "./paging";
import { percentile, summarizeFrames } from "./stats";
import { useKeysetRows, useRowTotal } from "./useKeysetRows";

const ROW_HEIGHT = 32;
const WHEEL_PX = 48;
const DRAG_PX = 1200;

/** Fixed seed, so every run scrolls the same way. */
function mulberry32(seed: number) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = seed;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Frames at the start with no scrolling: the control for webview and compositor jitter. */
const IDLE_FRAMES = 300;

/** One scrollTop per frame: an idle control phase, wheel-speed scroll, top → bottom → top at drag speed, then random
 * jumps each followed by a short wheel-speed scroll. */
function* scrollScript(maxTop: number): Generator<number> {
  const rand = mulberry32(0x5_2d);
  let y = 0;
  for (let i = 0; i < IDLE_FRAMES; i++) yield y;
  for (let i = 0; i < 600; i++) yield (y = Math.min(maxTop, y + WHEEL_PX));
  while (y < maxTop) yield (y = Math.min(maxTop, y + DRAG_PX));
  while (y > 0) yield (y = Math.max(0, y - DRAG_PX));
  for (let j = 0; j < 40; j++) {
    yield (y = Math.floor(rand() * maxTop));
    for (let i = 0; i < 20; i++) yield (y = Math.min(maxTop, y + WHEEL_PX));
  }
}

export function S2dScreen({ autorun }: { autorun: boolean }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const fetchTimes = useRef<number[]>([]);
  const [status, setStatus] = useState("Idle");
  const [stats, setStats] = useState<FrameStats | null>(null);
  const [running, setRunning] = useState(false);

  const onFetch = (ms: number) => fetchTimes.current.push(ms);
  const total = useRowTotal(onFetch);
  // eslint-disable-next-line react-hooks/incompatible-library -- TanStack Virtual is not memo-safe; nothing here is memoized.
  const virtualizer = useVirtualizer({
    count: total,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 10,
  });
  const items = virtualizer.getVirtualItems();
  const rowAt = useKeysetRows(items[0]?.index ?? 0, items.at(-1)?.index ?? 0, onFetch);

  const run = () => {
    const el = scrollRef.current;
    if (!el || running) return;
    setRunning(true);
    setStatus("Scrolling…");
    fetchTimes.current = [];
    const script = scrollScript(el.scrollHeight - el.clientHeight);
    const deltas: number[] = [];
    let last = 0;
    const frame = (now: number) => {
      if (last) deltas.push(now - last);
      last = now;
      const next = script.next();
      if (!next.done) {
        el.scrollTop = next.value;
        requestAnimationFrame(frame);
        return;
      }
      const fetches = [...fetchTimes.current].sort((a, b) => a - b);
      const idle = summarizeFrames(deltas.slice(0, IDLE_FRAMES));
      const summary = summarizeFrames(deltas.slice(IDLE_FRAMES));
      const result: FrameStats = {
        ...summary,
        idleMeanMs: idle.meanMs,
        idleP95Ms: idle.p95Ms,
        idleJankFrameShare: idle.jankFrameShare,
        pageFetches: fetches.length,
        pageFetchP50Ms: percentile(fetches, 50),
        pageFetchP95Ms: percentile(fetches, 95),
        rows: total,
        pageSize: PAGE_SIZE,
        userAgent: navigator.userAgent,
        devicePixelRatio: window.devicePixelRatio,
        viewport: `${String(el.clientWidth)}x${String(el.clientHeight)}`,
      };
      setStats(result);
      setRunning(false);
      commands.recordFrameStats(result).then(
        (path) => {
          setStatus(`Saved to ${path}`);
        },
        (e: unknown) => {
          setStatus(`Could not save: ${String(e)}`);
        },
      );
    };
    requestAnimationFrame(frame);
  };

  const runRef = useRef(run);
  runRef.current = run;
  const started = useRef(false);
  useEffect(() => {
    if (autorun && total > 0 && !started.current) {
      started.current = true;
      // Let the first page paint before measuring.
      setTimeout(() => {
        runRef.current();
      }, 1000);
    }
  }, [autorun, total]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-4 border-b border-border px-4 py-2 text-sm">
        <span>
          Spike S2d: {total.toLocaleString()} rows, pages of {PAGE_SIZE}
        </span>
        <button
          type="button"
          className="rounded bg-accent px-3 py-1 text-accent-text disabled:opacity-50"
          onClick={run}
          disabled={running || total === 0}
        >
          Run scroll test
        </button>
        <span className="text-muted" role="status">
          {status}
        </span>
        {stats && (
          <span className="text-muted">
            p95 {stats.p95Ms.toFixed(1)} ms · over 25 ms {(stats.jankFrameShare * 100).toFixed(2)} %
          </span>
        )}
      </div>
      <div
        ref={scrollRef}
        className="min-h-0 flex-1 overflow-auto"
        role="table"
        aria-rowcount={total}
        tabIndex={0}
      >
        <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
          {items.map((item) => {
            const row = rowAt(item.index);
            return (
              <div
                key={item.key}
                role="row"
                aria-rowindex={item.index + 1}
                className="absolute left-0 top-0 grid w-full grid-cols-[5rem_18rem_5rem_4rem_1fr_1fr] items-center gap-3 border-b border-border px-4 text-sm"
                style={{ height: ROW_HEIGHT, transform: `translateY(${String(item.start)}px)` }}
              >
                {row ? (
                  <>
                    <span role="cell" className="text-muted">
                      {row.key}
                    </span>
                    <span role="cell" className="truncate font-mono">
                      {row.identifier}
                    </span>
                    <span role="cell">{row.kind}</span>
                    <span role="cell">{row.sourceCount}</span>
                    <span role="cell" className="truncate text-muted">
                      {row.firstSource}
                    </span>
                    <span role="cell" className="truncate text-muted">
                      {row.secondSource}
                    </span>
                  </>
                ) : (
                  <span role="cell" className="text-muted">
                    …
                  </span>
                )}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

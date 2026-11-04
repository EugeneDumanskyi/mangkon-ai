import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AiActivityInfo } from "../ipc/bindings/AiActivityInfo";
import type { AiReadinessInfo } from "../ipc/bindings/AiReadinessInfo";
import type { RuntimeModel } from "../ipc/bindings/RuntimeModel";
import type { UiEvent } from "../ipc/bindings/UiEvent";
import { NO_MODEL_ALLOWED, runtimeLine } from "../lib/ai";
import { staleKeys } from "../lib/events";
import { AiReadinessView } from "../project/EgressSettings";
import { ProgressSummary } from "../project/ProgressPanel";
import { StatusBarView } from "./StatusBar";

const none = () => undefined;

const bar = (
  over: Partial<{
    activity: AiActivityInfo | null;
    readiness: AiReadinessInfo[];
    models: RuntimeModel[];
  }>,
) =>
  renderToStaticMarkup(
    <StatusBarView
      activity={over.activity ?? null}
      readiness={over.readiness ?? []}
      models={over.models ?? []}
      retrying={false}
      error={null}
      onRetry={none}
      onSettings={none}
    />,
  );

const activity = (host: string, cls: AiActivityInfo["class"], label: string): AiActivityInfo => ({
  host,
  class: cls,
  purpose: "extraction",
  label,
});

const blocked = (code: string, reason: string): AiReadinessInfo => ({
  role: "extraction",
  ready: false,
  destination: null,
  class: null,
  name: null,
  code,
  reason,
});

const model = (over: Partial<RuntimeModel>): RuntimeModel => ({
  modelId: "qwen3.5-4b-q4_k_m",
  role: "extraction",
  state: "ready",
  failure: null,
  port: 41234,
  restarts: 0,
  tail: "",
  ...over,
});

describe("status bar (US-09.8 AC1, NFR-SEC-05)", () => {
  it("AC1: says where data goes while a call is on its way, for each class", () => {
    for (const [a, text] of [
      [
        activity("127.0.0.1", "thisMachine", "Data sent to this machine"),
        "Data sent to this machine",
      ],
      [
        activity("192.168.1.20", "localNetwork", "Data sent to 192.168.1.20 (local network)"),
        "Data sent to 192.168.1.20 (local network)",
      ],
      [
        activity("api.openai.com", "internet", "Data sent to api.openai.com (internet)"),
        "Data sent to api.openai.com (internet)",
      ],
    ] as const) {
      const html = bar({ activity: a, readiness: [blocked("no_model", NO_MODEL_ALLOWED)] });
      expect(html).toContain(text);
      expect(html).toContain(`data-activity="${a.class}"`);
      expect(html).not.toContain(NO_MODEL_ALLOWED);
    }
  });

  it("the activity comes from the event, not a query", () => {
    const e: UiEvent = { kind: "aiActivity", activity: null };
    expect(staleKeys(e)).toEqual([]);
    expect(staleKeys({ kind: "runtimeChanged" })).toEqual([["runtime"], ["ai_readiness"]]);
  });
});

describe("no model allowed (US-09.7 AC3)", () => {
  it("AC3: shows the reason with a Settings link", () => {
    const html = bar({ readiness: [blocked("no_model", NO_MODEL_ALLOWED)] });
    expect(html).toContain(`People and organizations: ${NO_MODEL_ALLOWED}`);
    expect(html).toMatch(/<button[^>]*>Settings<\/button>/);
  });

  it("a runtime failure has no Settings link; Retry shows for a failed model", () => {
    const html = bar({
      readiness: [blocked("runtime_failed", "Not enough GPU memory")],
      models: [
        model({
          state: "failed",
          port: null,
          failure: { code: "gpu_memory", message: "Not enough GPU memory" },
        }),
      ],
    });
    expect(html).toContain("People and organizations: Not enough GPU memory");
    expect(html).not.toContain(">Settings<");
    expect(html).toContain(">Retry<");
  });

  it("project settings list each role's readiness", () => {
    const html = renderToStaticMarkup(
      <AiReadinessView
        readiness={[
          {
            role: "extraction",
            ready: true,
            destination: "Data sent to this machine",
            class: "thisMachine",
            name: "Qwen3.5 4B",
            code: null,
            reason: null,
          },
          { ...blocked("no_model", NO_MODEL_ALLOWED), role: "embeddings" },
        ]}
      />,
    );
    expect(html).toContain("AI for this project");
    expect(html).toContain("Qwen3.5 4B · Data sent to this machine");
    expect(html).toContain(`Meaning search</dt><dd>${NO_MODEL_ALLOWED}`);
  });
});

describe("local runtime state (US-09.5)", () => {
  it("shows the state and the loopback port; nothing while stopped", () => {
    expect(runtimeLine(model({}))).toBe("People and organizations: Ready · 127.0.0.1:41234");
    expect(runtimeLine(model({ state: "starting", port: null }))).toBe(
      "People and organizations: Starting…",
    );
    expect(runtimeLine(model({ state: "stopped" }))).toBeNull();
    expect(bar({ models: [model({ state: "stopped" })] })).not.toContain("data-runtime");
    expect(bar({ models: [model({})] })).not.toContain(">Retry<");
  });

  it("US-09.5 AC3: the progress panel says why AI work waits", () => {
    const html = renderToStaticMarkup(
      <ProgressSummary
        progress={{
          total: 3,
          done: 3,
          failed: 0,
          queued: 0,
          notProcessed: 0,
          ingesting: false,
          paused: false,
          stage: null,
          runDone: 3,
          runTotal: 3,
          currentFiles: [],
          eta: { kind: "none" },
          blocked: "Not enough GPU memory",
        }}
      />,
    );
    expect(html).toContain("AI work waits: Not enough GPU memory");
  });
});

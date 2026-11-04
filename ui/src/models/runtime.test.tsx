import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AiChoiceInfo } from "../ipc/bindings/AiChoiceInfo";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { RuntimeInfo } from "../ipc/bindings/RuntimeInfo";
import { ModelRowView } from "./LocalModels";
import { AiChoiceSectionView, RuntimeSectionView } from "./Runtime";

const none = () => undefined;

const info = (over: Partial<RuntimeInfo>): RuntimeInfo => ({
  path: "/opt/llama.cpp/build/bin/llama-server",
  source: "settings",
  build: 10863,
  commit: "88ada91c1",
  version: "0.4.0-dev",
  minBuild: 10863,
  devices: [
    {
      id: "CUDA0",
      backend: "CUDA",
      description: "NVIDIA GeForce RTX 5070 Laptop GPU (7707 MiB, 7570 MiB free)",
    },
  ],
  backends: ["CUDA"],
  error: null,
  settingsPath: "/opt/llama.cpp/build/bin/llama-server",
  installGuide: "docs/install-llama-cpp.md",
  backendNote: null,
  models: [],
  ...over,
});

const section = (i: RuntimeInfo) =>
  renderToStaticMarkup(
    <RuntimeSectionView
      info={i}
      busy={false}
      error={null}
      onChoose={none}
      onClear={none}
      onCheck={none}
      onRetry={none}
    />,
  );

describe("llama-server (ADR-0015)", () => {
  it("shows the path, where it came from, the build and the backend", () => {
    const html = section(info({}));
    expect(html).toContain("/opt/llama.cpp/build/bin/llama-server (chosen in settings)");
    expect(html).toContain("10863 (commit 88ada91c1)");
    expect(html).toContain('data-backend="true">CUDA<');
    expect(html).toContain("NVIDIA GeForce RTX 5070 Laptop GPU");
    expect(html).toContain("Choose…");
    expect(html).toContain("Use PATH");
    expect(html).toContain("Check again");
  });

  it("not found or too old: the reason and the install guide", () => {
    const html = section(
      info({
        path: null,
        source: null,
        build: null,
        commit: null,
        devices: [],
        backends: [],
        settingsPath: null,
        error: { code: "not_found", message: "llama-server not found" },
      }),
    );
    expect(html).toContain("Not found");
    expect(html).toContain("llama-server not found. See docs/install-llama-cpp.md.");
    expect(html).not.toContain("Use PATH");
  });

  it("the CPU-only hint, a failed model with Retry and its details", () => {
    const html = section(
      info({
        backends: ["CPU only"],
        backendNote: "This machine has a GPU (RTX), but this llama-server build runs on the CPU",
        models: [
          {
            modelId: "qwen",
            role: "extraction",
            state: "failed",
            failure: { code: "gpu_memory", message: "Not enough GPU memory" },
            port: null,
            restarts: 0,
            tail: "cudaMalloc failed: out of memory",
          },
        ],
      }),
    );
    expect(html).toContain("runs on the CPU");
    expect(html).toContain("Not enough GPU memory");
    expect(html).toContain(">Retry<");
    expect(html).toContain("Show details");
    // Details stay closed until asked.
    expect(html).not.toContain("cudaMalloc");
  });
});

const catalog = (over: Partial<CatalogModel>): CatalogModel => ({
  id: "qwen",
  name: "Qwen3.5 4B",
  role: "extraction",
  quant: "Q4_K_M",
  bytes: 2.9e9,
  vramBytes: 3.5e9,
  ramBytes: 3.3e9,
  license: "Apache-2.0",
  licenseUrl: "https://example.org/LICENSE",
  recommended: true,
  fits: true,
  diskNeeded: null,
  state: "installed",
  bytesDone: 2.9e9,
  speedBps: 0,
  etaS: null,
  error: null,
  ...over,
});

describe("Try and Used for AI (US-09.5 AC1, INV-MM-04)", () => {
  it("an installed model has Try, and shows what Try found", () => {
    const html = renderToStaticMarkup(
      <ModelRowView
        model={catalog({})}
        busy={false}
        error={null}
        onStart={none}
        onPause={none}
        onResume={none}
        onCancel={none}
        onDelete={none}
        onTry={none}
        tryResult={{
          modelId: "qwen",
          role: "extraction",
          ok: true,
          summary: "Connected: qwen answered in 812 ms",
          bound: "127.0.0.1:41234",
          startMs: 4200,
          responseMs: 812,
          structuredOutput: true,
          dimensions: null,
        }}
      />,
    );
    expect(html).toContain(">Try<");
    expect(html).toContain("Connected: qwen answered in 812 ms · ran on 127.0.0.1:41234");
    expect(html).toContain("started in 4.2 s");
  });

  it("each role lists the default, installed models of its role and the endpoints", () => {
    const choices: AiChoiceInfo[] = [
      {
        role: "extraction",
        choice: { kind: "local", id: "qwen" },
        explicit: false,
        name: "Qwen3.5 4B",
      },
      { role: "embeddings", choice: null, explicit: false, name: null },
    ];
    const html = renderToStaticMarkup(
      <AiChoiceSectionView
        choices={choices}
        models={[
          catalog({}),
          catalog({ id: "emb", name: "Embed", role: "embeddings", state: "available" }),
        ]}
        endpoints={[]}
        busy={false}
        error={null}
        onSet={none}
      />,
    );
    expect(html).toContain("Default: Qwen3.5 4B");
    expect(html).toContain("None: no model");
    expect(html).toContain('value="local:qwen"');
    expect(html).not.toContain('value="local:emb"');
  });
});

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { HardwareInfo } from "../ipc/bindings/HardwareInfo";
import { ChooseModelStep, FirstProjectStep, StepList } from "./FirstRun";

const none = () => undefined;

const hardware = (over: Partial<HardwareInfo>): HardwareInfo => ({
  os: "Linux",
  cpu: "Test CPU",
  ramBytes: 64e9,
  unifiedBytes: null,
  gpus: [],
  freeDiskBytes: 100e9,
  diskPath: "/models",
  meets: true,
  label: "Meets minimum",
  reasons: [],
  notes: [],
  tier: "minimum",
  localOverride: false,
  localAllowed: true,
  tooWeak: null,
  overrideWarning: "",
  ...over,
});

const model = (over: Partial<CatalogModel>): CatalogModel => ({
  id: "qwen-4b",
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
  state: "available",
  bytesDone: 0,
  speedBps: 0,
  etaS: null,
  error: null,
  ...over,
});

const choose = (hw: HardwareInfo, models: CatalogModel[], saved: string | null = null) =>
  renderToStaticMarkup(
    <ChooseModelStep
      hardware={hw}
      models={models}
      busy={null}
      errors={{}}
      endpointSaved={saved}
      onAction={none}
      onEndpoint={none}
      onNext={none}
    />,
  );

describe("first run (US-14.4)", () => {
  it("AC1: three steps, the current one marked", () => {
    const html = renderToStaticMarkup(<StepList step={2} />);
    expect(html).toContain('aria-label="Step 2 of 3"');
    expect(html).toContain("1. This machine");
    expect(html).toMatch(/aria-current="step"[^>]*>2\. AI model/);
    expect(html).toContain("3. First project");
  });

  it("AC1: step 2 offers the recommended models, an endpoint, or no AI", () => {
    const html = choose(hardware({}), [
      model({}),
      model({ id: "other", name: "Other", recommended: false }),
    ]);
    expect(html).toContain("Qwen3.5 4B");
    expect(html).not.toContain(">Other<");
    expect(html).toContain(">Download<");
    expect(html).toContain("Use an endpoint…");
    expect(html).toContain("Skip — use without AI");
    expect(html).not.toContain(">Try<");
  });

  it("AC1: below the minimum, only the endpoint or no AI", () => {
    const html = choose(
      hardware({
        meets: false,
        localAllowed: false,
        tooWeak: "This machine is too weak for local AI. Connect an external model endpoint.",
      }),
      [model({})],
    );
    expect(html).not.toContain("Qwen3.5 4B");
    expect(html).not.toContain(">Download<");
    expect(html).toContain("too weak for local AI");
    expect(html).toContain("Use an endpoint…");
    expect(html).toContain("Skip — use without AI");
  });

  it("Next once a model is installed or an endpoint is set", () => {
    expect(choose(hardware({}), [model({ state: "installed" })])).toContain(">Next<");
    const html = choose(hardware({ localAllowed: false }), [], "Box");
    expect(html).toContain("Box is used for people and organizations.");
    expect(html).toContain(">Next<");
  });

  it("AC2: step 3 offers the demo project found next to the app, or a folder to choose", () => {
    const found = renderToStaticMarkup(
      <FirstProjectStep demo="/repo/testdata/td1" busy={false} demoError={null} onDemo={none}>
        <p>form</p>
      </FirstProjectStep>,
    );
    expect(found).toContain("/repo/testdata/td1");
    expect(found).toContain("Open demo project");
    expect(found).toContain("<p>form</p>");
    const missing = renderToStaticMarkup(
      <FirstProjectStep demo={null} busy={false} demoError="No demo files" onDemo={none}>
        {null}
      </FirstProjectStep>,
    );
    expect(missing).toContain("Choose the demo folder…");
    expect(missing).toContain("No demo files");
  });
});

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { HardwareInfo } from "../ipc/bindings/HardwareInfo";
import { gb } from "../lib/models";
import { CatalogSection, HardwareSection } from "./LocalModels";

const none = () => undefined;

const TOO_WEAK = "This machine is too weak for local AI. Connect an external model endpoint.";

export const hardware = (over: Partial<HardwareInfo>): HardwareInfo => ({
  os: "Linux (Pop!_OS 24.04)",
  cpu: "Test CPU (24 threads)",
  ramBytes: 64e9,
  unifiedBytes: null,
  gpus: [
    {
      vendor: "NVIDIA",
      name: "NVIDIA GeForce RTX 5070 Laptop GPU",
      vramBytes: 8_546_942_976,
      driver: "595.104.02",
      used: true,
    },
  ],
  freeDiskBytes: 412e9,
  diskPath: "/home/u/.local/share/mangkon-ai/models",
  meets: true,
  label: "Meets minimum",
  reasons: [],
  notes: [],
  tier: "large-ram",
  localOverride: false,
  localAllowed: true,
  tooWeak: null,
  overrideWarning: "Not supported. It may be very slow.",
  ...over,
});

const below = hardware({
  ramBytes: 8e9,
  gpus: [{ vendor: "NVIDIA", name: "GTX 1060", vramBytes: 6e9, driver: "550.1", used: true }],
  meets: false,
  label: "Below minimum",
  reasons: ["GPU GTX 1060: VRAM 6 GB, needs 8 GB", "RAM 8 GB, needs 16 GB"],
  localAllowed: false,
  tooWeak: TOO_WEAK,
});

const section = (info: HardwareInfo | undefined) =>
  renderToStaticMarkup(
    <HardwareSection info={info} checking={false} error={null} onCheck={none} onOverride={none} />,
  );

const model: CatalogModel = {
  id: "emb",
  name: "Test embeddings",
  role: "embeddings",
  quant: "Q8_0",
  bytes: 146e6,
  vramBytes: 0.5e9,
  ramBytes: 0.8e9,
  license: "Apache-2.0",
  licenseUrl: "https://www.apache.org/licenses/LICENSE-2.0",
  recommended: true,
  fits: true,
  diskNeeded: null,
  state: "available",
  bytesDone: 0,
  speedBps: 0,
  etaS: null,
  error: null,
};

const catalog = (info: HardwareInfo) =>
  renderToStaticMarkup(
    <CatalogSection hardware={info} models={[model]} busy={null} errors={{}} onAction={none} />,
  );

describe("This machine (US-09.1)", () => {
  it("AC1: shows OS, CPU, RAM, each GPU with VRAM and driver, and free disk", () => {
    const html = section(hardware({}));
    expect(html).toContain("Linux (Pop!_OS 24.04)");
    expect(html).toContain("Test CPU (24 threads)");
    expect(html).toContain("64 GB");
    expect(html).toContain("NVIDIA GeForce RTX 5070 Laptop GPU · VRAM 8.5 GB · driver 595.104.02");
    expect(html).toContain("412 GB");
    expect(html).toContain("Check hardware");
  });

  it("AC1: unified memory on Apple Silicon, and a GPU that was not found", () => {
    const html = section(hardware({ unifiedBytes: 32e9, gpus: [] }));
    expect(html).toContain("32 GB unified");
    expect(html).toContain("Not detected");
  });

  it("AC2: the verdict and each reason", () => {
    expect(section(hardware({}))).toContain('data-verdict="meets"');
    const html = section(below);
    expect(html).toContain('data-verdict="below"');
    expect(html).toContain("Below minimum");
    expect(html).toContain("GPU GTX 1060: VRAM 6 GB, needs 8 GB");
    expect(html).toContain("RAM 8 GB, needs 16 GB");
  });

  it("AC3/AC4: notes are shown", () => {
    const html = section(hardware({ notes: ["Supported on a best-effort basis (Vulkan)"] }));
    expect(html).toContain("Supported on a best-effort basis (Vulkan)");
  });

  it("GB are 10^9 bytes", () => {
    expect(gb(8_546_942_976)).toBe("8.5");
    expect(gb(8e9)).toBe("8");
    expect(gb(100.9e9)).toBe("101");
  });
});

describe("Below minimum (US-09.2)", () => {
  it("AC1: the sentence, a way to the endpoints, and no model downloads", () => {
    const html = section(below);
    expect(html).toContain(TOO_WEAK);
    expect(html).toContain('href="#endpoints"');
    const list = catalog(below);
    expect(list).toContain("data-not-offered");
    expect(list).not.toContain("Download");
  });

  it("AC3: the override sits under Advanced; once on, it shows the warning", () => {
    expect(section(below)).toContain("Advanced");
    expect(section(hardware({}))).not.toContain("Advanced");
    const on = { ...below, localOverride: true, localAllowed: true, tooWeak: null };
    const html = section(on);
    expect(html).toContain("Allow local models anyway");
    expect(html).toContain("Not supported. It may be very slow.");
    expect(html).toContain("checked");
    expect(catalog(on)).toContain("Download");
  });
});

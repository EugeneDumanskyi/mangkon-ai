import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import { staleKeys } from "../lib/events";
import { size, timeLeft } from "../lib/models";
import { ModelRowView } from "./LocalModels";

const none = () => undefined;

const model = (over: Partial<CatalogModel>): CatalogModel => ({
  id: "qwen-4b",
  name: "Qwen3.5 4B",
  role: "extraction",
  quant: "Q4_K_M",
  bytes: 2.9e9,
  vramBytes: 3.5e9,
  ramBytes: 3.3e9,
  license: "Apache-2.0",
  licenseUrl: "https://huggingface.co/Qwen/Qwen3.5-4B/blob/main/LICENSE",
  recommended: false,
  fits: true,
  diskNeeded: null,
  state: "available",
  bytesDone: 0,
  speedBps: 0,
  etaS: null,
  error: null,
  ...over,
});

const row = (m: CatalogModel, error: string | null = null) =>
  renderToStaticMarkup(
    <ModelRowView
      model={m}
      busy={false}
      error={error}
      onStart={none}
      onPause={none}
      onResume={none}
      onCancel={none}
      onDelete={none}
    />,
  );

/** The row's action buttons (not the license link). */
const buttons = (html: string) =>
  [...html.matchAll(/<button[^>]*class="rounded[^"]*"[^>]*>([^<]*)<\/button>/g)].map((x) => x[1]);

describe("Models (US-09.3)", () => {
  it("AC1: name, role, size, memory needed and the Recommended badge", () => {
    const html = row(model({ recommended: true }));
    expect(html).toContain("Qwen3.5 4B");
    expect(html).toContain("People and organizations");
    expect(html).toContain("2.9 GB on disk");
    expect(html).toContain("needs 3.5 GB VRAM and 3.3 GB RAM");
    expect(html).toContain("Recommended");
    expect(row(model({}))).not.toContain("Recommended");
    expect(row(model({ fits: false }))).toContain("more than this machine has");
    expect(row(model({ diskNeeded: 4.9e9 }))).toContain("Does not fit: needs 4.9 GB free disk");
    expect(row(model({}))).not.toContain("Does not fit");
  });

  it("AC3: the license (SPDX and link) is shown before Download", () => {
    const html = row(model({}));
    const license = html.indexOf("License: Apache-2.0");
    expect(license).toBeGreaterThan(-1);
    expect(html).toContain("https://huggingface.co/Qwen/Qwen3.5-4B/blob/main/LICENSE");
    expect(buttons(html)).toContain("Download");
  });
});

describe("Downloads (US-09.4)", () => {
  it("AC1: the disk check message is shown on the row", () => {
    const msg = "Not enough free disk space: this download needs 30 GB free, 12 GB is free.";
    expect(row(model({}), msg)).toContain(msg);
  });

  it("AC2: progress, speed and time left, with Pause and Cancel", () => {
    const html = row(
      model({ state: "downloading", bytesDone: 1.45e9, speedBps: 31.8e6, etaS: 45 }),
    );
    expect(html).toContain("<progress");
    expect(html).toContain('value="50"');
    expect(html).toContain("1.4 GB of 2.9 GB (50 %)");
    expect(html).toContain("31.8 MB/s");
    expect(html).toContain("45 s left");
    expect(buttons(html)).toEqual(["Pause", "Cancel"]);
  });

  it("AC3: a paused or interrupted download offers Resume and Cancel", () => {
    for (const state of ["paused", "interrupted"] as const) {
      const html = row(model({ state, bytesDone: 1e9 }));
      expect(buttons(html)).toEqual(["Resume", "Cancel"]);
      expect(html).toContain("1 GB of 2.9 GB");
    }
    const lost = row(
      model({
        state: "interrupted",
        bytesDone: 1e9,
        error: {
          code: "network",
          message: "The network connection was lost. Resume to continue.",
        },
      }),
    );
    expect(lost).toContain("The network connection was lost. Resume to continue.");
  });

  it("AC4: a checksum mismatch says so and offers Download again", () => {
    const html = row(
      model({ error: { code: "corrupted", message: "Download corrupted. Please retry." } }),
    );
    expect(html).toContain("Download corrupted. Please retry.");
    expect(buttons(html)).toEqual(["Download"]);
  });

  it("AC5: an installed model offers Delete", () => {
    const html = row(model({ state: "installed", bytesDone: 2.9e9 }));
    expect(html).toContain("Installed");
    expect(buttons(html)).toEqual(["Delete"]);
    expect(html).not.toContain("<progress");
  });

  it("verifying shows the check with no buttons", () => {
    const html = row(model({ state: "verifying", bytesDone: 2.9e9 }));
    expect(html).toContain("Checking the download…");
    expect(buttons(html)).toEqual([]);
  });

  it("models events re-query the hardware, the catalog and what AI can run", () => {
    expect(staleKeys({ kind: "modelsChanged" })).toEqual([
      ["hardware"],
      ["model_catalog"],
      ["ai_choice"],
      ["ai_readiness"],
    ]);
  });

  it("sizes and times", () => {
    expect(size(146e6)).toBe("146 MB");
    expect(size(37e9)).toBe("37 GB");
    expect(timeLeft(200)).toBe("3 min 20 s");
    expect(timeLeft(3900)).toBe("1 h 5 min");
  });
});

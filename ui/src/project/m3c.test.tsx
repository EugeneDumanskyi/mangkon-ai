import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DocumentItem } from "../ipc/bindings/DocumentItem";
import type { Progress } from "../ipc/bindings/Progress";
import type { RecentProject } from "../ipc/bindings/RecentProject";
import { DocumentDetails, DocumentRowView } from "./DocumentsList";
import { ProgressSummary } from "./ProgressPanel";
import { SettingsForm, SourceRootRow } from "./SettingsPanel";
import { MissingProject, NewProjectForm, RecentList } from "./StartScreen";
import { createErrorField, ocrChoices, parseDomains } from "../lib/forms";

const none = () => undefined;

const recent = (over: Partial<RecentProject>): RecentProject => ({
  id: 1,
  name: "Case",
  path: "/p/Case",
  lastOpenedAt: 0,
  present: true,
  ...over,
});

describe("start screen (US-01.1, US-01.2)", () => {
  it("lists recent projects in the order given and marks a missing one", () => {
    const html = renderToStaticMarkup(
      <RecentList
        projects={[
          recent({ id: 2, name: "Newer" }),
          recent({ id: 1, name: "Older", present: false }),
        ]}
        disabled={false}
        onOpen={none}
      />,
    );
    expect(html.indexOf("Newer")).toBeLessThan(html.indexOf("Older"));
    expect(html).toContain("(not found)");
  });

  it("offers Locate… and Remove from list for a missing project", () => {
    const html = renderToStaticMarkup(
      <MissingProject
        project={recent({ path: "/gone/Case", present: false })}
        busy={false}
        error={null}
        onLocate={none}
        onForget={none}
        onClose={none}
      />,
    );
    expect(html).toContain("Project not found at");
    expect(html).toContain("/gone/Case");
    expect(html).toContain("Locate…");
    expect(html).toContain("Remove from list");
  });

  it("shows create errors next to their field", () => {
    expect(createErrorField("name_taken")).toBe("name");
    expect(createErrorField("not_writable")).toBe("location");
    expect(createErrorField("project")).toBeNull();
    const html = renderToStaticMarkup(
      <NewProjectForm
        location="/ro"
        pending={false}
        error={{ code: "not_writable", message: "Cannot write to this folder" }}
        onPickLocation={none}
        onCreate={none}
        onCancel={none}
      />,
    );
    expect(html).toContain("Cannot write to this folder");
    expect(html).toContain("/ro");
  });
});

const progress = (over: Partial<Progress>): Progress => ({
  total: 10,
  done: 4,
  failed: 1,
  queued: 5,
  notProcessed: 0,
  ingesting: true,
  paused: false,
  stage: "A",
  runDone: 3,
  runTotal: 8,
  currentFiles: ["/src/big.pdf"],
  eta: { kind: "estimating" },
  ...over,
});

describe("progress panel (US-03.1, US-03.2)", () => {
  it("shows stage, files, current file and Estimating…", () => {
    const html = renderToStaticMarkup(<ProgressSummary progress={progress({})} />);
    expect(html).toContain("Stage A");
    expect(html).toContain("3 of 8 files");
    expect(html).toContain("Now: big.pdf");
    expect(html).toContain("Estimating…");
  });

  it("shows a paused queue as paused, without a current file", () => {
    const html = renderToStaticMarkup(
      <ProgressSummary
        progress={progress({ paused: true, ingesting: false, eta: { kind: "none" } })}
      />,
    );
    expect(html).toContain("Paused");
    expect(html).not.toContain("Now:");
  });
});

const doc = (over: Partial<DocumentItem>): DocumentItem => ({
  refId: 1,
  contentId: 1,
  path: "/src/scan.pdf",
  fileType: "pdf",
  size: 2048,
  units: 3,
  unit: "pages",
  status: "done",
  warnings: [],
  failureReason: null,
  failureDetail: null,
  otherPaths: 0,
  ...over,
});

describe("documents list (US-02.8, US-02.7)", () => {
  it("shows name, path, type, size, units and status", () => {
    const html = renderToStaticMarkup(
      <DocumentRowView row={doc({})} checked={false} onCheck={none} onDetails={none} />,
    );
    for (const s of ["scan.pdf", "/src/scan.pdf", "pdf", "2.0 KB", "3 pages", "Done"]) {
      expect(html).toContain(s);
    }
    expect(html).not.toContain("Show details");
  });

  it("shows a failed file's reason, details and Retry", () => {
    const failed = doc({
      status: "failed",
      failureReason: "The file is damaged",
      failureDetail: "Error code: pdf_damaged",
    });
    const row = renderToStaticMarkup(
      <DocumentRowView row={failed} checked={false} onCheck={none} onDetails={none} />,
    );
    expect(row).toContain("The file is damaged");
    expect(row).toContain("Show details");
    const details = renderToStaticMarkup(
      <DocumentDetails row={failed} retrying={false} onRetry={none} onClose={none} />,
    );
    expect(details).toContain("Error code: pdf_damaged");
    expect(details).toContain("Retry");
  });
});

describe("settings (US-01.6, D12)", () => {
  it("reads the stop list one per line or comma-separated", () => {
    expect(parseDomains("gmail.com\n\n outlook.com, yahoo.com ")).toEqual([
      "gmail.com",
      "outlook.com",
      "yahoo.com",
    ]);
  });

  it("offers installed OCR languages and marks a chosen one that is not installed", () => {
    expect(ocrChoices(["eng", "deu"], ["eng", "fra"])).toEqual(["eng", "deu", "fra"]);
    const html = renderToStaticMarkup(
      <SettingsForm
        settings={{
          phoneRegion: null,
          ocrLanguages: ["eng", "fra"],
          domainStopList: ["gmail.com"],
        }}
        installed={["eng"]}
        saving={false}
        onSave={none}
      />,
    );
    expect(html).toContain("(not installed)");
    expect(html).toContain("gmail.com");
  });

  it("marks a source that is not found and offers Locate…", () => {
    const html = renderToStaticMarkup(
      <SourceRootRow
        root={{ id: 1, path: "/old/src", kind: "folder", present: false, excluded: 2 }}
        busy={false}
        onLocate={none}
        onRemove={none}
      />,
    );
    expect(html).toContain("not found");
    expect(html).toContain("2 paths removed");
    expect(html).toContain("Locate…");
  });
});

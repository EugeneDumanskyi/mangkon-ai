import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { originalNote } from "../lib/labels";
import { OriginalBanner } from "./Original";
import { MentionNav, OriginalActions } from "./SourcePane";

const none = () => undefined;
const withClient = (node: ReactNode) =>
  renderToStaticMarkup(
    <QueryClientProvider client={new QueryClient()}>{node}</QueryClientProvider>,
  );

describe("mention position (US-08.2 AC1)", () => {
  it('shows "Mention 2 of 5" with Previous and Next', () => {
    const html = renderToStaticMarkup(
      <MentionNav position={{ index: 2, count: 5, prev: 10, next: 12 }} onMention={none} />,
    );
    expect(html).toContain("Mention 2 of 5");
    expect(html).toContain(">Previous<");
    expect(html).toContain(">Next<");
    expect(html).not.toContain(`disabled=""`);
  });

  it("turns off Previous on the first and Next on the last", () => {
    const first = renderToStaticMarkup(
      <MentionNav position={{ index: 1, count: 2, prev: null, next: 12 }} onMention={none} />,
    );
    expect(first).toMatch(/disabled=""[^>]*data-prev/);
    const last = renderToStaticMarkup(
      <MentionNav position={{ index: 2, count: 2, prev: 10, next: null }} onMention={none} />,
    );
    expect(last).toMatch(/disabled=""[^>]*data-next/);
  });

  it("shows nothing for the only mention, or a document view", () => {
    expect(
      renderToStaticMarkup(
        <MentionNav position={{ index: 1, count: 1, prev: null, next: null }} onMention={none} />,
      ),
    ).toBe("");
    expect(renderToStaticMarkup(<MentionNav position={null} onMention={none} />)).toBe("");
  });
});

describe("original file (US-08.5, US-08.6)", () => {
  it('says "Original file not found at <path>" when it is gone', () => {
    const note = originalNote({ state: "missing", path: "/cases/a.eml" });
    expect(note).toBe("Original file not found at /cases/a.eml");
    const html = renderToStaticMarkup(<OriginalBanner note={note ?? ""} />);
    expect(html).toContain('role="status"');
    expect(html).toContain("Original file not found at /cases/a.eml");
  });

  it("says when it changed, and nothing when it is there", () => {
    expect(originalNote({ state: "changed", path: "/a" })).toBe(
      "The file changed since it was read: showing the stored text",
    );
    expect(originalNote({ state: "present", path: "/a" })).toBeNull();
  });

  it("offers Show in folder and Open with default app, off when the file is gone", () => {
    const there = withClient(
      <OriginalActions documentId={1} original={{ state: "present", path: "/a" }} />,
    );
    expect(there).toContain("Show in folder");
    expect(there).toContain("Open with default app");
    expect(there).not.toContain(`disabled=""`);
    const gone = withClient(
      <OriginalActions documentId={1} original={{ state: "missing", path: "/a" }} />,
    );
    expect(gone.match(/disabled=""/g)).toHaveLength(2);
    const changed = withClient(
      <OriginalActions documentId={1} original={{ state: "changed", path: "/a" }} />,
    );
    expect(changed).not.toContain(`disabled=""`);
  });
});

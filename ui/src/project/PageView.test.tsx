import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { PageView } from "../ipc/bindings/PageView";
import { ocrQualityLabel } from "../lib/labels";
import { PageFigure } from "./PageView";
import type { PageBody } from "./SourceBody";

// US-08.1 AC1–AC2, US-08.3, ADR-0011: the page image with the hit's boxes, the stored text on
// the toggle, and the stored text with a note when the page cannot be shown.

const hostile = '<script>alert(1)</script><img src="http://track.example/p.gif">';

const body: PageBody = {
  kind: "page",
  page: 2,
  pages: 3,
  text: "ocr",
  lines: [{ before: `${hostile} `, hit: "x@y.example", after: "" }],
  ocrQuality: 0.42,
};

const shown: PageView = {
  documentId: 7,
  page: 2,
  pages: 4,
  status: "shown",
  width: 1200,
  height: 1600,
  boxes: [
    { x: 0.1, y: 0.25, width: 0.2, height: 0.025 },
    { x: 0.32, y: 0.25, width: 0.1, height: 0.025 },
  ],
  note: null,
};

function render(page: PageView | undefined, showText: boolean, url: string | null = "blob:p") {
  return renderToStaticMarkup(
    <PageFigure body={body} page={page} url={url} showText={showText} onToggle={() => undefined} />,
  );
}

describe("page view", () => {
  it("shows the page image with one box per word, placed as fractions", () => {
    const html = render(shown, false);
    expect(html).toContain('src="blob:p"');
    expect(html).toContain("Page 2 of 4");
    expect(html.match(/data-hit/g)).toHaveLength(2);
    expect(html).toContain("left:10%;top:25%;width:20%;height:2.5%");
    expect(html).toContain("Show OCR text");
    expect(html).not.toContain("x@y.example");
  });

  it("toggles to the stored text with the hit and the OCR quality", () => {
    const html = render(shown, true);
    expect(html).not.toContain("<img");
    expect(html).toContain("Show page");
    expect(html).toMatch(/<mark data-hit/);
    expect(html).toContain("OCR quality 42% (low)");
    expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(html).not.toMatch(/<script/i);
  });

  it("names the text layer for a PDF text page", () => {
    const html = renderToStaticMarkup(
      <PageFigure
        body={{ ...body, text: "pdf", ocrQuality: null }}
        page={shown}
        url="blob:p"
        showText={false}
        onToggle={() => undefined}
      />,
    );
    expect(html).toContain("Show text");
    expect(html).not.toContain("OCR quality");
  });

  for (const status of ["originalMissing", "changed", "textOnly"] as const) {
    it(`falls back to the stored text with the note when the page is ${status}`, () => {
      const note = "Original file missing: showing the stored text";
      const html = render({ ...shown, status, width: 0, height: 0, boxes: [], note }, false);
      expect(html).not.toContain("<img");
      expect(html).not.toContain("data-toggle-text");
      expect(html).toContain(note);
      expect(html).toContain("<mark");
    });
  }

  it("waits for the page and its image", () => {
    expect(render(undefined, false)).toContain("Loading the page");
    expect(render(shown, false, null)).toContain("Loading the page");
  });

  it("labels OCR quality", () => {
    expect(ocrQualityLabel(0.874)).toBe("OCR quality 87%");
    expect(ocrQualityLabel(0.59)).toBe("OCR quality 59% (low)");
  });
});

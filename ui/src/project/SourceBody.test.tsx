import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { MarkedText } from "../ipc/bindings/MarkedText";
import type { SourceBody } from "../ipc/bindings/SourceBody";
import { LinkContext } from "../lib/linkContext";
import { SourceBodyView } from "./SourceBody";

// US-08.4 AC1–AC2, NFR-SEC-09: document text with markup in it is shown as text. No element
// from a document reaches the page, so no script runs and nothing loads from the network.
const hostile = '<script>alert(1)</script><img src="http://track.example/p.gif">';

const marked = (text: string): MarkedText => ({
  before: text,
  hit: "x@y.example",
  after: text,
  runs: [],
});

const bodies: SourceBody[] = [
  { kind: "text", lines: [{ number: 1, text: marked(hostile) }] },
  {
    kind: "table",
    header: [hostile],
    rows: [{ row: 1, cells: [marked(hostile)], malformed: false }],
  },
  { kind: "paragraphs", paragraphs: [marked(hostile)] },
  { kind: "json", rows: [{ depth: 0, label: hostile, value: marked(hostile) }] },
  {
    kind: "email",
    headers: [{ name: hostile, value: marked(hostile) }],
    parts: [{ part: "1", lines: [marked(hostile)] }],
    attachments: [{ documentId: 2, label: hostile }],
  },
];

describe("source views", () => {
  for (const body of bodies) {
    it(`shows markup in a ${body.kind} body as text only`, () => {
      const html = renderToStaticMarkup(
        <SourceBodyView body={body} onOpenDocument={() => undefined} />,
      );
      expect(html).not.toMatch(/<script|<img|<iframe|<link|<style/i);
      expect(html).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
      expect(html).toContain("&lt;img src=&quot;http://track.example/p.gif&quot;&gt;");
      expect(html).toContain("<mark");
    });
  }
});

describe("links in document text (US-08.4 AC3)", () => {
  const withLink: MarkedText = {
    before: "Mail mailto:",
    hit: "x@y.example",
    after: " or see https://example.org/a",
    runs: [
      { kind: "text", text: "Mail ", hit: false },
      {
        kind: "link",
        url: "mailto:x@y.example",
        pieces: [
          { text: "mailto:", hit: false },
          { text: "x@y.example", hit: true },
        ],
      },
      { kind: "text", text: " or see ", hit: false },
      {
        kind: "link",
        url: "https://example.org/a",
        pieces: [{ text: "https://example.org/a", hit: false }],
      },
    ],
  };
  const body: SourceBody = { kind: "paragraphs", paragraphs: [withLink] };

  it("shows a link as a button, never as an anchor, the hit kept inside it", () => {
    const html = renderToStaticMarkup(
      <LinkContext.Provider value={() => undefined}>
        <SourceBodyView body={body} onOpenDocument={() => undefined} />
      </LinkContext.Provider>,
    );
    expect(html).not.toMatch(/<a[\s>]|href=/i);
    expect(html).toContain('data-link="https://example.org/a"');
    expect(html).toMatch(/<button[^>]*data-link="mailto:x@y.example"[^>]*>mailto:<mark/);
  });

  it("shows links as plain text where nothing can open them", () => {
    const html = renderToStaticMarkup(
      <SourceBodyView body={body} onOpenDocument={() => undefined} />,
    );
    expect(html).not.toContain("<button");
    expect(html).toContain("https://example.org/a");
    expect(html).toContain("<mark");
  });
});

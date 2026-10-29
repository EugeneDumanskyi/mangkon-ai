import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { MarkedText } from "../ipc/bindings/MarkedText";
import type { SourceBody } from "../ipc/bindings/SourceBody";
import { SourceBodyView } from "./SourceBody";

// US-08.4 AC1–AC2, NFR-SEC-09: document text with markup in it is shown as text. No element
// from a document reaches the page, so no script runs and nothing loads from the network.
const hostile = '<script>alert(1)</script><img src="http://track.example/p.gif">';

const marked = (text: string): MarkedText => ({ before: text, hit: "x@y.example", after: text });

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

import type { DocumentLink } from "../ipc/bindings/DocumentLink";
import type { MarkedText } from "../ipc/bindings/MarkedText";
import type { SourceBody } from "../ipc/bindings/SourceBody";
import { LinkRun, PieceText } from "./Links";

// Every document text is shown as React text nodes only: no HTML from a document is ever parsed,
// so nothing in it runs or loads (US-08.4, NFR-SEC-09, gap A8).

/** A text with its hit, already split on the Rust side; with its links as runs, if any. */
export function Marked({ text }: { text: MarkedText }) {
  if (text.runs.length > 0) {
    return (
      <>
        {text.runs.map((r, i) =>
          r.kind === "link" ? (
            <LinkRun key={i} url={r.url} pieces={r.pieces} />
          ) : (
            <PieceText key={i} piece={r} />
          ),
        )}
      </>
    );
  }
  return (
    <>
      {text.before}
      {text.hit !== "" && (
        <mark data-hit className="rounded-sm bg-accent px-0.5 text-accent-text">
          {text.hit}
        </mark>
      )}
      {text.after}
    </>
  );
}

function Lines({ lines }: { lines: { number: number; text: MarkedText }[] }) {
  return (
    <table className="w-full border-collapse font-mono text-sm">
      <tbody>
        {lines.map((l) => (
          <tr key={l.number}>
            <td className="select-none pr-3 text-right align-top text-muted">{l.number}</td>
            <td className="whitespace-pre-wrap break-all">
              <Marked text={l.text} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export type PageBody = Extract<SourceBody, { kind: "page" }>;

/** The stored text of a page (US-08.3): OCR or text-layer lines, the hit marked. */
export function PageLines({ body }: { body: PageBody }) {
  if (body.text === null) {
    return <p className="text-sm text-muted">No text was found on this page.</p>;
  }
  return (
    <div className="whitespace-pre-wrap break-words font-mono text-sm" data-page-text>
      {body.lines.map((l, i) => (
        <div key={i}>
          <Marked text={l} />
          {l.before === "" && l.hit === "" && l.after === "" ? " " : null}
        </div>
      ))}
    </div>
  );
}

/** A link to another document of the same file: an attachment or its email. */
export function DocumentButton({
  link,
  onOpen,
  prefix,
}: {
  link: DocumentLink;
  onOpen: (documentId: number) => void;
  prefix?: string;
}) {
  return (
    <button
      type="button"
      className="text-left underline decoration-dotted hover:text-accent"
      data-document-id={link.documentId}
      onClick={() => {
        onOpen(link.documentId);
      }}
    >
      {prefix}
      {link.label}
    </button>
  );
}

/** The body of a source view, by document kind (US-08.1 AC3–AC6). */
export function SourceBodyView({
  body,
  onOpenDocument,
}: {
  body: SourceBody;
  onOpenDocument: (documentId: number) => void;
}) {
  switch (body.kind) {
    case "text":
      return <Lines lines={body.lines} />;
    case "paragraphs":
      return (
        <div className="max-w-prose space-y-3 text-sm">
          {body.paragraphs.map((p, i) => (
            <p key={i} className="whitespace-pre-wrap break-words">
              <Marked text={p} />
            </p>
          ))}
        </div>
      );
    case "json":
      return (
        <ul className="font-mono text-sm">
          {body.rows.map((r, i) => (
            <li key={i} style={{ paddingLeft: `${String(r.depth * 1.25)}rem` }}>
              <span className="text-muted">{r.label}</span>
              {r.value && (
                <>
                  <span className="text-muted">: </span>
                  <span className="whitespace-pre-wrap break-all">
                    <Marked text={r.value} />
                  </span>
                </>
              )}
            </li>
          ))}
        </ul>
      );
    case "email":
      return (
        <div className="space-y-4 text-sm">
          <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
            {body.headers.map((h, i) => (
              <div key={i} className="contents">
                <dt className="text-muted">{h.name}</dt>
                <dd className="break-all">
                  <Marked text={h.value} />
                </dd>
              </div>
            ))}
          </dl>
          {body.parts.map((p) => (
            <div key={p.part} className="border-t border-border pt-3">
              {body.parts.length > 1 && <p className="pb-1 text-muted">Part {p.part}</p>}
              <div className="whitespace-pre-wrap break-words">
                {p.lines.map((l, i) => (
                  <div key={i}>
                    <Marked text={l} />
                    {l.before === "" && l.hit === "" && l.after === "" ? " " : null}
                  </div>
                ))}
              </div>
            </div>
          ))}
          {body.attachments.length > 0 && (
            <div className="border-t border-border pt-3">
              <p className="pb-1 text-muted">Attachments</p>
              <ul>
                {body.attachments.map((a) => (
                  <li key={a.documentId}>
                    <DocumentButton link={a} onOpen={onOpenDocument} />
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      );
    case "page":
      return <PageLines body={body} />;
    case "table":
      return (
        <table className="border-collapse text-sm">
          {body.header && (
            <thead>
              <tr>
                <th className="border border-border px-2 py-1 text-left text-muted">row</th>
                {body.header.map((h, i) => (
                  <th key={i} className="border border-border px-2 py-1 text-left">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
          )}
          <tbody>
            {body.rows.map((r) => (
              <tr key={r.row}>
                <td className="border border-border px-2 py-1 text-right text-muted">
                  {r.row}
                  {r.malformed ? " (wrong number of columns)" : ""}
                </td>
                {r.cells.map((c, i) => (
                  <td
                    key={i}
                    className={`border border-border px-2 py-1 whitespace-pre ${
                      c.hit !== "" ? "outline-2 -outline-offset-2 outline-accent" : ""
                    }`}
                  >
                    <Marked text={c} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      );
  }
}

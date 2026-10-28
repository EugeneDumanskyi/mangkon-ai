import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { MarkedText } from "../ipc/bindings/MarkedText";
import type { SourceView } from "../ipc/bindings/SourceView";
import { fileName, locationLabel } from "../lib/labels";

/** A text with its hit, already split on the Rust side; React text nodes only (NFR-SEC-09). */
function Marked({ text }: { text: MarkedText }) {
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

function Body({ view }: { view: SourceView }) {
  const { body } = view;
  if (body.kind === "text") {
    return (
      <table className="w-full border-collapse font-mono text-sm">
        <tbody>
          {body.lines.map((l) => (
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

/** The source around one mention, the hit highlighted and scrolled into view (US-08.1). */
export function SourcePane({ mentionId }: { mentionId: number | null }) {
  const ref = useRef<HTMLDivElement>(null);
  const view = useQuery({
    queryKey: ["source", mentionId],
    queryFn: () => commands.sourceContext(mentionId ?? 0),
    enabled: mentionId !== null,
  });

  useEffect(() => {
    ref.current?.querySelector("[data-hit]")?.scrollIntoView({ block: "center", inline: "center" });
  }, [view.data]);

  if (mentionId === null) {
    return (
      <section className="p-3 text-sm text-muted" aria-label="Source">
        Pick a mention to see it in its file.
      </section>
    );
  }
  const o = view.data?.occurrence;
  return (
    <section className="flex min-h-0 flex-col" aria-label="Source">
      <h2 className="border-b border-border px-3 py-2 text-sm">
        {o ? (
          <>
            <span className="font-semibold" title={o.path ?? undefined}>
              {fileName(o.path)}
            </span>
            <span className="text-muted"> · {locationLabel(o.location)}</span>
          </>
        ) : (
          "Source"
        )}
      </h2>
      {view.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(view.error)}
        </p>
      )}
      <div ref={ref} className="min-h-0 flex-1 overflow-auto p-3">
        {view.data && <Body view={view.data} />}
      </div>
    </section>
  );
}

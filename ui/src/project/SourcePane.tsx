import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { SourceView } from "../ipc/bindings/SourceView";
import { fileName, locationLabel } from "../lib/labels";
import { PageView } from "./PageView";
import { DocumentButton, SourceBodyView } from "./SourceBody";

/** Where the view is: at a mention, or at a document opened from a link. */
function placeLabel(view: SourceView): string {
  if (view.occurrence) return locationLabel(view.occurrence.location);
  return view.document.position === "whole" ? "start of file" : view.document.position;
}

/**
 * The source around one mention, the hit highlighted and scrolled into view (US-08.1). An
 * attachment links back to its email, and an email to its attachments (AC4).
 */
export function SourcePane({ mentionId }: { mentionId: number | null }) {
  const ref = useRef<HTMLDivElement>(null);
  // A document opened from a link; cleared when another mention is picked.
  const [opened, setOpened] = useState<{ mentionId: number | null; documentId: number } | null>(
    null,
  );
  const documentId = opened?.mentionId === mentionId ? opened.documentId : null;
  const view = useQuery({
    queryKey: ["source", mentionId, documentId],
    queryFn: () =>
      documentId !== null
        ? commands.sourceDocument(documentId)
        : commands.sourceContext(mentionId ?? 0),
    enabled: mentionId !== null,
  });
  const open = (id: number) => {
    setOpened({ mentionId, documentId: id });
  };

  useEffect(() => {
    const hit = ref.current?.querySelector("[data-hit]");
    if (hit) hit.scrollIntoView({ block: "center", inline: "center" });
    else ref.current?.scrollTo({ top: 0 });
  }, [view.data]);

  if (mentionId === null) {
    return (
      <section className="p-3 text-sm text-muted" aria-label="Source">
        Pick a mention to see it in its file.
      </section>
    );
  }
  const v = view.data;
  // Which view is on screen, for the scripted view sweep (`viewSweep.ts`).
  const shown = documentId !== null ? `d${String(documentId)}` : `m${String(mentionId)}`;
  return (
    <section
      className="flex min-h-0 flex-col"
      aria-label="Source"
      data-view={v ? shown : view.error ? `${shown}-error` : undefined}
      data-kind={v?.body.kind}
    >
      <h2 className="border-b border-border px-3 py-2 text-sm">
        {v ? (
          <>
            <span className="font-semibold" title={v.document.path ?? undefined}>
              {fileName(v.document.path)}
            </span>
            <span className="text-muted"> · {placeLabel(v)}</span>
          </>
        ) : (
          "Source"
        )}
      </h2>
      {v?.parent && (
        <p className="border-b border-border px-3 py-1 text-sm">
          <DocumentButton link={v.parent} onOpen={open} prefix="Attachment of " />
        </p>
      )}
      {documentId !== null && (
        <p className="border-b border-border px-3 py-1 text-sm">
          <button
            type="button"
            className="underline decoration-dotted hover:text-accent"
            data-back
            onClick={() => {
              setOpened(null);
            }}
          >
            Back to the mention
          </button>
        </p>
      )}
      {view.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(view.error)}
        </p>
      )}
      <div ref={ref} className="min-h-0 flex-1 overflow-auto p-3">
        {v?.body.kind === "page" ? (
          <PageView
            key={`${String(v.document.documentId)}:${String(v.body.page)}:${String(mentionId)}`}
            documentId={v.document.documentId}
            mentionId={v.occurrence?.mentionId ?? null}
            body={v.body}
          />
        ) : (
          v && <SourceBodyView body={v.body} onOpenDocument={open} />
        )}
      </div>
    </section>
  );
}

import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { MentionPosition } from "../ipc/bindings/MentionPosition";
import type { OriginalFile } from "../ipc/bindings/OriginalFile";
import type { SourceView } from "../ipc/bindings/SourceView";
import { fileName, locationLabel, originalNote } from "../lib/labels";
import { LinkContext } from "../lib/linkContext";
import { LinkConfirm } from "./Links";
import { OriginalBanner } from "./Original";
import { PageView } from "./PageView";
import { DocumentButton, SourceBodyView } from "./SourceBody";

/** Where the view is: at a mention, or at a document opened from a link. */
function placeLabel(view: SourceView): string {
  if (view.occurrence) return locationLabel(view.occurrence.location);
  return view.document.position === "whole" ? "start of file" : view.document.position;
}

/** "Mention 2 of 5" with Previous and Next, when the value is in the document more than once. */
export function MentionNav({
  position,
  onMention,
}: {
  position: MentionPosition | null;
  onMention: (mentionId: number) => void;
}) {
  if (position === null || position.count < 2) return null;
  const step = "rounded border border-border px-2 py-0.5 disabled:opacity-50";
  return (
    <span className="flex items-center gap-2" data-position>
      <span>
        Mention {position.index} of {position.count}
      </span>
      <button
        type="button"
        className={step}
        disabled={position.prev === null}
        data-prev
        onClick={() => {
          if (position.prev !== null) onMention(position.prev);
        }}
      >
        Previous
      </button>
      <button
        type="button"
        className={step}
        disabled={position.next === null}
        data-next
        onClick={() => {
          if (position.next !== null) onMention(position.next);
        }}
      >
        Next
      </button>
    </span>
  );
}

/** Show in folder and Open with default app (US-08.6); off when the original is not there. */
export function OriginalActions({
  documentId,
  original,
}: {
  documentId: number;
  original: OriginalFile | null;
}) {
  const reveal = useMutation({ mutationFn: () => commands.showInFolder(documentId) });
  const open = useMutation({ mutationFn: () => commands.openOriginal(documentId) });
  const gone = original === null || original.state === "missing";
  const button = "rounded border border-border px-2 py-0.5 disabled:opacity-50";
  const error = reveal.error ?? open.error;
  return (
    <span className="flex flex-wrap items-center gap-2">
      <button
        type="button"
        className={button}
        disabled={gone}
        onClick={() => {
          reveal.mutate();
        }}
      >
        Show in folder
      </button>
      <button
        type="button"
        className={button}
        disabled={gone}
        onClick={() => {
          open.mutate();
        }}
      >
        Open with default app
      </button>
      {error && <span role="alert">{errorMessage(error)}</span>}
    </span>
  );
}

/**
 * The source around one mention, the hit highlighted and scrolled into view (US-08.1), or a
 * document opened from a link. An attachment links back to its email, and an email to its
 * attachments (AC4). Moving to another mention or document, and back, goes through the project
 * screen's history (US-08.2).
 */
export function SourcePane({
  mentionId,
  documentId,
  canBack,
  onBack,
  onMention,
  onOpenDocument,
}: {
  mentionId: number | null;
  /** A document opened from a link, shown instead of the mention. */
  documentId: number | null;
  canBack: boolean;
  onBack: () => void;
  onMention: (mentionId: number) => void;
  onOpenDocument: (documentId: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [link, setLink] = useState<string | null>(null);
  const openLink = useMutation({
    mutationFn: (url: string) => commands.openLink(url),
    onSuccess: () => {
      setLink(null);
    },
  });
  const view = useQuery({
    queryKey: ["source", mentionId, documentId],
    queryFn: () =>
      documentId !== null
        ? commands.sourceDocument(documentId)
        : commands.sourceContext(mentionId ?? 0),
    enabled: mentionId !== null || documentId !== null,
  });

  useEffect(() => {
    const hit = ref.current?.querySelector("[data-hit]");
    if (hit) hit.scrollIntoView({ block: "center", inline: "center" });
    else ref.current?.scrollTo({ top: 0 });
  }, [view.data]);

  if (mentionId === null && documentId === null) {
    return (
      <section className="p-3 text-sm text-muted" aria-label="Source">
        Pick a mention to see it in its file.
      </section>
    );
  }
  const v = view.data;
  const note = v ? originalNote(v.original) : null;
  // Which view is on screen, for the scripted sweeps (`viewSweep.ts`).
  const shown = documentId !== null ? `d${String(documentId)}` : `m${String(mentionId)}`;
  const bar =
    "flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-3 py-1 text-sm";
  return (
    <section
      className="flex min-h-0 flex-col"
      aria-label="Source"
      data-view={v ? shown : view.error ? `${shown}-error` : undefined}
      data-kind={v?.body.kind}
    >
      <h2 className="flex items-center gap-2 border-b border-border px-3 py-2 text-sm">
        {canBack && (
          <button
            type="button"
            className="rounded border border-border px-2 py-0.5"
            title="Back (Alt+←)"
            data-back
            onClick={onBack}
          >
            ← Back
          </button>
        )}
        {v ? (
          <span className="min-w-0 truncate">
            <span className="font-semibold" title={v.original?.path ?? undefined}>
              {fileName(v.original?.path ?? v.document.path)}
            </span>
            <span className="text-muted"> · {placeLabel(v)}</span>
          </span>
        ) : (
          "Source"
        )}
      </h2>
      {v && (
        <div className={bar}>
          <MentionNav position={v.position} onMention={onMention} />
          <OriginalActions documentId={v.document.documentId} original={v.original} />
        </div>
      )}
      {note !== null && (
        <div className="border-b border-border px-3 py-1">
          <OriginalBanner note={note} />
        </div>
      )}
      {v?.parent && (
        <p className={bar}>
          <DocumentButton link={v.parent} onOpen={onOpenDocument} prefix="Attachment of " />
        </p>
      )}
      {view.error && (
        <p role="alert" className="px-3 py-2 text-sm">
          {errorMessage(view.error)}
        </p>
      )}
      <LinkContext.Provider value={setLink}>
        <div ref={ref} className="min-h-0 flex-1 overflow-auto p-3" data-scroll="source">
          {v?.body.kind === "page" ? (
            <PageView
              key={`${String(v.document.documentId)}:${String(v.body.page)}:${String(mentionId)}`}
              documentId={v.document.documentId}
              mentionId={v.occurrence?.mentionId ?? null}
              body={v.body}
              bannerShown={note !== null}
            />
          ) : (
            v && <SourceBodyView body={v.body} onOpenDocument={onOpenDocument} />
          )}
        </div>
      </LinkContext.Provider>
      {link !== null && (
        <LinkConfirm
          url={link}
          error={openLink.error ? errorMessage(openLink.error) : null}
          onCancel={() => {
            setLink(null);
            openLink.reset();
          }}
          onOpen={() => {
            openLink.mutate(link);
          }}
        />
      )}
    </section>
  );
}

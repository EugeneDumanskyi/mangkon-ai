import { useContext, useEffect, useRef } from "react";
import type { TextPiece } from "../ipc/bindings/TextPiece";
import { LinkContext } from "../lib/linkContext";

// Links in document text (US-08.4 AC3). A link is a button, never an `<a href>`: a click asks
// first, showing the full URL, and only "Open" hands it to the core, which checks it again and
// passes it to the system browser or mail app. The navigation guard stays the backstop.

/** A piece of text, the hit marked. */
export function PieceText({ piece }: { piece: TextPiece }) {
  return piece.hit ? (
    <mark data-hit className="rounded-sm bg-accent px-0.5 text-accent-text">
      {piece.text}
    </mark>
  ) : (
    <>{piece.text}</>
  );
}

/** A URL found in the text, styled as a link. */
export function LinkRun({ url, pieces }: { url: string; pieces: TextPiece[] }) {
  const ask = useContext(LinkContext);
  const text = pieces.map((p, i) => <PieceText key={i} piece={p} />);
  if (ask === null) return <>{text}</>;
  return (
    <button
      type="button"
      className="inline break-all text-left text-accent underline hover:decoration-2"
      title={url}
      data-link={url}
      onClick={() => {
        ask(url);
      }}
    >
      {text}
    </button>
  );
}

/** "Open this link?" with the full URL; Cancel is the default (US-08.4 AC3). */
export function LinkConfirm({
  url,
  error,
  onCancel,
  onOpen,
}: {
  url: string;
  error: string | null;
  onCancel: () => void;
  onOpen: () => void;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    cancel.current?.focus();
  }, []);
  const mail = url.toLowerCase().startsWith("mailto:");
  return (
    <div
      className="fixed inset-0 z-10 flex items-center justify-center bg-black/40 p-4"
      onKeyDown={(e) => {
        if (e.key === "Escape") onCancel();
      }}
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="link-confirm-title"
        aria-describedby="link-confirm-url"
        className="w-full max-w-lg space-y-3 rounded border border-border bg-surface p-4 text-sm shadow-lg"
      >
        <h2 id="link-confirm-title" className="font-semibold">
          {mail ? "Open this address in your mail app?" : "Open this link in your browser?"}
        </h2>
        <p
          id="link-confirm-url"
          className="max-h-40 overflow-auto break-all rounded-sm bg-surface-alt px-2 py-1 font-mono"
          data-link-url
        >
          {url}
        </p>
        <p className="text-muted">
          It leaves Mangkon AI. Check the full address before you open it.
        </p>
        {error !== null && <p role="alert">{error}</p>}
        <div className="flex justify-end gap-2">
          <button
            ref={cancel}
            type="button"
            className="rounded border border-border px-3 py-1"
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className="rounded bg-accent px-3 py-1 text-accent-text"
            data-link-open
            onClick={onOpen}
          >
            Open
          </button>
        </div>
      </div>
    </div>
  );
}

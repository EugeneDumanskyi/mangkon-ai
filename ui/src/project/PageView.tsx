import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import { ocrQualityLabel } from "../lib/labels";
import type { PageView as Page } from "../ipc/bindings/PageView";
import type { SegmentHit } from "../ipc/bindings/SegmentHit";
import { type PageBody, PageLines } from "./SourceBody";
import { OriginalBanner } from "./Original";

// The page image comes from the core as PNG bytes and is shown through a `blob:` URL; the boxes
// are fractions of the page, so they stay on the hit at any size (US-08.1 AC1–AC2). The text
// side is the stored text as text nodes (US-08.3, US-08.4).

function percent(f: number): string {
  return `${String(Math.round(f * 1000) / 10)}%`;
}

/** A `blob:` URL for PNG bytes, revoked when the bytes change or the view goes. */
function useBlobUrl(bytes: ArrayBuffer | undefined): string | null {
  const url = useMemo(
    () => (bytes ? URL.createObjectURL(new Blob([bytes], { type: "image/png" })) : null),
    [bytes],
  );
  useEffect(
    () => () => {
      if (url) URL.revokeObjectURL(url);
    },
    [url],
  );
  return url;
}

/**
 * What the page view shows, without fetching anything: the page image with the hit's boxes,
 * or the stored text (toggled, or because the page cannot be shown, with the note why).
 */
export function PageFigure({
  body,
  page,
  url,
  showText,
  onToggle,
  onImageLoad,
  bannerShown = false,
}: {
  body: PageBody;
  /** `undefined` while it loads. */
  page: Page | undefined;
  /** The page image's URL, once loaded. */
  url: string | null;
  showText: boolean;
  onToggle: () => void;
  onImageLoad?: () => void;
  /** The pane already says the original is gone or changed (US-08.5). */
  bannerShown?: boolean;
}) {
  const shown = page?.status === "shown";
  const aboutOriginal = page?.status === "originalMissing" || page?.status === "changed";
  const textLabel = body.text === "ocr" ? "Show OCR text" : "Show text";
  const pages = page?.pages ?? body.pages;
  return (
    <div className="space-y-2" data-page-status={page?.status}>
      <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
        <span className="text-muted">
          Page {body.page} of {pages}
        </span>
        {shown && (
          <button
            type="button"
            className="underline decoration-dotted hover:text-accent"
            aria-pressed={showText}
            data-toggle-text
            onClick={onToggle}
          >
            {showText ? "Show page" : textLabel}
          </button>
        )}
        {body.ocrQuality !== null && (showText || !shown) && (
          <span className="text-muted">{ocrQualityLabel(body.ocrQuality)}</span>
        )}
      </p>
      {page?.note &&
        (aboutOriginal ? (
          // Folded into the pane's banner when it already says so.
          !bannerShown && <OriginalBanner note={page.note} />
        ) : (
          <p
            role="note"
            className="rounded-sm border border-border bg-surface-alt px-2 py-1 text-sm"
          >
            {page.note}
          </p>
        ))}
      {page === undefined ? (
        <p className="text-sm text-muted">Loading the page…</p>
      ) : shown && !showText ? (
        url === null ? (
          <p className="text-sm text-muted">Loading the page…</p>
        ) : (
          <div className="relative w-fit max-w-full border border-border">
            <img
              src={url}
              width={page.width}
              height={page.height}
              alt={`Page ${String(page.page)}`}
              className="block h-auto max-w-full"
              onLoad={onImageLoad}
            />
            {page.boxes.map((b, i) => (
              <span
                key={i}
                data-hit
                aria-hidden
                className="pointer-events-none absolute rounded-sm bg-accent/25 outline-2 outline-accent"
                style={{
                  left: percent(b.x),
                  top: percent(b.y),
                  width: percent(b.width),
                  height: percent(b.height),
                }}
              />
            ))}
          </div>
        )
      ) : (
        <PageLines body={body} />
      )}
    </div>
  );
}

/**
 * One page of a PDF or image at a mention, a search result or a document (US-08.1 AC1–AC2,
 * US-08.3, US-06.1 AC4). Keyed by the parent on document, page and place, so the toggle starts
 * on the image each time.
 */
export function PageView({
  documentId,
  mentionId,
  hit = null,
  body,
  bannerShown = false,
}: {
  documentId: number;
  mentionId: number | null;
  /** A search hit, when the view was opened at one. */
  hit?: SegmentHit | null;
  body: PageBody;
  bannerShown?: boolean;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [showText, setShowText] = useState(false);
  const page = useQuery({
    queryKey: ["page", documentId, body.page, mentionId, hit],
    queryFn: () => commands.pageView(documentId, body.page, mentionId, hit),
  });
  const shown = page.data?.status === "shown";
  const image = useQuery({
    queryKey: ["pageImage", documentId, body.page],
    queryFn: () => commands.pageImage(documentId, body.page),
    enabled: shown,
    staleTime: Infinity,
  });
  const url = useBlobUrl(image.data);
  const failed = page.error ?? image.error;
  const scrollToHit = () => {
    ref.current?.querySelector("[data-hit]")?.scrollIntoView({ block: "center", inline: "center" });
  };
  useEffect(() => {
    ref.current?.querySelector("[data-hit]")?.scrollIntoView({ block: "center", inline: "center" });
  }, [showText]);

  return (
    <div ref={ref}>
      {failed && (
        <p role="alert" className="pb-2 text-sm">
          {errorMessage(failed)}
        </p>
      )}
      <PageFigure
        body={body}
        page={failed ? textOnly(documentId, body) : page.data}
        url={url}
        showText={showText}
        onToggle={() => {
          setShowText((s) => !s);
        }}
        onImageLoad={scrollToHit}
        bannerShown={bannerShown}
      />
    </div>
  );
}

/** The text-only view when the page or its image could not be had. */
function textOnly(documentId: number, body: PageBody): Page {
  return {
    documentId,
    page: body.page,
    pages: body.pages,
    status: "textOnly",
    width: 0,
    height: 0,
    boxes: [],
    note: null,
  };
}

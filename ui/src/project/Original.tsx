/** Why the view shows stored text only: the original is gone or changed (US-08.5). */
export function OriginalBanner({ note }: { note: string }) {
  return (
    <p
      role="status"
      className="rounded-sm border border-border bg-surface-alt px-2 py-1 text-sm"
      data-original-banner
    >
      {note}
    </p>
  );
}

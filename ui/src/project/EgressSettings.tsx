import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { AllowPrompt } from "../ipc/bindings/AllowPrompt";
import type { EgressRecord } from "../ipc/bindings/EgressRecord";
import type { ProjectEndpoint } from "../ipc/bindings/ProjectEndpoint";
import { accessLabel, classLabel, destinationLine, purposeLabel, shownTime } from "../lib/egress";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** One endpoint in the project's "AI endpoints" section (US-09.7). */
export function ProjectEndpointRow({
  endpoint,
  busy,
  onAllow,
  onRevoke,
}: {
  endpoint: ProjectEndpoint;
  busy: boolean;
  onAllow: () => void;
  onRevoke: () => void;
}) {
  const e = endpoint;
  const kind = e.access.kind;
  return (
    <li className="flex flex-col gap-0.5 border-b border-border py-2" data-endpoint={e.id}>
      <div className="flex items-baseline gap-3">
        <span className="font-semibold">{e.name}</span>
        <span className="truncate font-mono text-sm">{e.baseUrl}</span>
        <span className="ml-auto flex shrink-0 gap-2">
          {(kind === "notAllowed" || kind === "needsReconfirmation") && (
            <button type="button" className={button} disabled={busy} onClick={onAllow}>
              Allow for this project…
            </button>
          )}
          {(kind === "allowed" || kind === "needsReconfirmation") && (
            <button type="button" className={button} disabled={busy} onClick={onRevoke}>
              Revoke
            </button>
          )}
        </span>
      </div>
      <span className="text-sm" data-access={kind}>
        {accessLabel(e)}
      </span>
      <span className="text-sm text-muted">
        {e.destination !== null
          ? destinationLine(e.destination)
          : `Cannot resolve now: ${e.destinationError ?? "unknown"}`}
      </span>
    </li>
  );
}

/**
 * The confirm dialog (US-09.7 AC2, INV-EG-02): the exact host, port, class and addresses.
 * Cancel comes first and has the focus.
 */
export function AllowDialog({
  prompt,
  error,
  busy,
  onCancel,
  onConfirm,
}: {
  prompt: AllowPrompt;
  error: string | null;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    cancel.current?.focus();
  }, []);
  const d = prompt.destination;
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
        aria-labelledby="allow-title"
        aria-describedby="allow-destination"
        className="w-full max-w-lg space-y-3 rounded border border-border bg-surface p-4 text-sm shadow-lg"
      >
        <h2 id="allow-title" className="font-semibold">
          Send this project&apos;s data to {prompt.name}?
        </h2>
        <dl id="allow-destination" className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          <dt className="text-muted">Host</dt>
          <dd className="font-mono" data-allow-host>
            {d.host}:{d.port}
          </dd>
          <dt className="text-muted">Where</dt>
          <dd data-allow-class>{classLabel(d.class)}</dd>
          <dt className="text-muted">Addresses</dt>
          <dd className="font-mono">{d.addresses.join(", ")}</dd>
          <dt className="text-muted">Connection</dt>
          <dd>{d.encrypted ? "Encrypted (https)" : "Not encrypted (http)"}</dd>
        </dl>
        <p className="text-muted">
          AI extraction and search in this project will send text from its files to this endpoint.
          Other projects are not affected. If the host later points somewhere else, you are asked
          again.
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
            className="rounded border border-border px-3 py-1 font-semibold"
            disabled={busy}
            onClick={onConfirm}
          >
            Allow {d.host} ({classLabel(d.class)})
          </button>
        </div>
      </div>
    </div>
  );
}

/** "Data sent from this project" (US-09.8 AC2). */
export function EgressRecordsTable({ records }: { records: EgressRecord[] }) {
  if (records.length === 0) {
    return <p className="text-sm text-muted">Nothing from this project has been sent anywhere.</p>;
  }
  return (
    <table className="max-w-4xl text-left text-sm">
      <thead>
        <tr className="text-muted">
          <th className="pr-4 font-normal">Host</th>
          <th className="pr-4 font-normal">Where</th>
          <th className="pr-4 font-normal">For</th>
          <th className="pr-4 font-normal">First sent</th>
          <th className="font-normal">Last sent</th>
        </tr>
      </thead>
      <tbody>
        {records.map((r) => (
          <tr key={`${r.host} ${r.class} ${r.purpose}`} className="border-t border-border">
            <td className="pr-4 font-mono">{r.host}</td>
            <td className="pr-4">{classLabel(r.class)}</td>
            <td className="pr-4">{purposeLabel(r.purpose)}</td>
            <td className="pr-4">{shownTime(r.firstSentAt)}</td>
            <td>{shownTime(r.lastSentAt)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** The project's AI endpoint permissions and its egress records (US-09.7, US-09.8). */
export function EgressSettings() {
  const client = useQueryClient();
  const endpoints = useQuery({
    queryKey: ["project_endpoints"],
    queryFn: commands.projectEndpoints,
  });
  const records = useQuery({ queryKey: ["egress_records"], queryFn: commands.egressRecords });
  const [prompt, setPrompt] = useState<AllowPrompt | null>(null);
  const changed = () => {
    void client.invalidateQueries({ queryKey: ["project_endpoints"] });
  };
  const prepare = useMutation({
    mutationFn: commands.endpointAllowPrepare,
    onSuccess: (p) => {
      if (p.needsConfirmation) setPrompt(p);
      else changed();
    },
  });
  const confirm = useMutation({
    mutationFn: (p: AllowPrompt) =>
      commands.endpointAllowConfirm(p.endpointId, p.destination.origin, p.destination.class),
    onSuccess: () => {
      setPrompt(null);
      changed();
    },
  });
  const revoke = useMutation({ mutationFn: commands.endpointRevoke, onSuccess: changed });
  const error = endpoints.error ?? prepare.error ?? revoke.error;
  return (
    <>
      <section aria-label="AI endpoints" className="mt-6 flex flex-col gap-2">
        <h3 className="font-semibold">AI endpoints</h3>
        <p className="max-w-2xl text-sm text-muted">
          Endpoints on another computer or on the internet get nothing from this project until you
          allow them here. Endpoints are added on the AI models screen.
        </p>
        {error && (
          <p role="alert" className="text-sm">
            {errorMessage(error)}
          </p>
        )}
        <ul className="max-w-4xl">
          {(endpoints.data ?? []).map((e) => (
            <ProjectEndpointRow
              key={e.id}
              endpoint={e}
              busy={prepare.isPending || revoke.isPending}
              onAllow={() => {
                confirm.reset();
                prepare.mutate(e.id);
              }}
              onRevoke={() => {
                revoke.mutate(e.id);
              }}
            />
          ))}
        </ul>
        {endpoints.data?.length === 0 && (
          <p className="text-sm text-muted">No endpoints. Add one on the AI models screen.</p>
        )}
      </section>
      <section aria-label="Data sent from this project" className="mt-6 flex flex-col gap-2">
        <h3 className="font-semibold">Data sent from this project</h3>
        {records.error && (
          <p role="alert" className="text-sm">
            {errorMessage(records.error)}
          </p>
        )}
        {records.data && <EgressRecordsTable records={records.data} />}
      </section>
      {prompt !== null && (
        <AllowDialog
          prompt={prompt}
          busy={confirm.isPending}
          error={confirm.error ? errorMessage(confirm.error) : null}
          onCancel={() => {
            setPrompt(null);
          }}
          onConfirm={() => {
            confirm.mutate(prompt);
          }}
        />
      )}
    </>
  );
}

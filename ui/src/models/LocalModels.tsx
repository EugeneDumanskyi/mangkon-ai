import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { HardwareInfo } from "../ipc/bindings/HardwareInfo";
import { gb, percent, roleLabel, size, speed, stateLabel, timeLeft } from "../lib/models";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";

/** "This machine" (US-09.1): what the check found, the verdict and the override (US-09.2). */
export function HardwareSection({
  info,
  checking,
  error,
  onCheck,
  onOverride,
}: {
  info: HardwareInfo | undefined;
  checking: boolean;
  error: string | null;
  onCheck: () => void;
  onOverride: (on: boolean) => void;
}) {
  const [advanced, setAdvanced] = useState(false);
  return (
    <section aria-label="This machine" className="mt-4 flex flex-col gap-2">
      <div className="flex items-baseline gap-3">
        <h3 className="font-semibold">This machine</h3>
        <button type="button" className={button} disabled={checking} onClick={onCheck}>
          {checking ? "Checking…" : "Check hardware"}
        </button>
      </div>
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      {info === undefined ? (
        !error && <p className="text-sm text-muted">Checking this machine…</p>
      ) : (
        <>
          <dl className="grid max-w-2xl grid-cols-[8rem_1fr] gap-x-3 gap-y-0.5 text-sm">
            <dt className="text-muted">System</dt>
            <dd>{info.os}</dd>
            <dt className="text-muted">CPU</dt>
            <dd>{info.cpu}</dd>
            {info.unifiedBytes !== null ? (
              <>
                <dt className="text-muted">Memory</dt>
                <dd>{gb(info.unifiedBytes)} GB unified</dd>
              </>
            ) : (
              <>
                <dt className="text-muted">RAM</dt>
                <dd>{gb(info.ramBytes)} GB</dd>
              </>
            )}
            {info.gpus.length === 0 && (
              <>
                <dt className="text-muted">GPU</dt>
                <dd>Not detected</dd>
              </>
            )}
            {info.gpus.map((g, i) => (
              <div key={i} className="contents" data-gpu>
                <dt className="text-muted">GPU</dt>
                <dd>
                  {g.name}
                  {g.vramBytes !== null ? ` · VRAM ${gb(g.vramBytes)} GB` : " · VRAM unknown"}
                  {g.driver !== null && ` · driver ${g.driver}`}
                </dd>
              </div>
            ))}
            <dt className="text-muted">Free disk</dt>
            <dd>
              {info.freeDiskBytes !== null ? `${gb(info.freeDiskBytes)} GB` : "Unknown"}
              <span className="text-muted"> · {info.diskPath}</span>
            </dd>
          </dl>
          <p className="flex items-baseline gap-2">
            <span
              data-verdict={info.meets ? "meets" : "below"}
              className={`rounded border px-2 py-0.5 text-sm font-semibold ${
                info.meets ? "border-border" : "border-current"
              }`}
            >
              {info.label}
            </span>
          </p>
          {(info.reasons.length > 0 || info.notes.length > 0) && (
            <ul className="max-w-2xl list-disc pl-5 text-sm">
              {info.reasons.map((r) => (
                <li key={r}>{r}</li>
              ))}
              {info.notes.map((n) => (
                <li key={n} data-note>
                  {n}
                </li>
              ))}
            </ul>
          )}
          {info.tooWeak !== null && (
            <p role="note" className="max-w-2xl text-sm">
              {info.tooWeak} <a href="#endpoints">Endpoints</a>
            </p>
          )}
          {!info.meets && (
            <div className="text-sm">
              <button
                type="button"
                className="underline"
                aria-expanded={advanced || info.localOverride}
                onClick={() => {
                  setAdvanced(!advanced);
                }}
              >
                Advanced
              </button>
              {(advanced || info.localOverride) && (
                <label className="mt-1 flex items-center gap-1">
                  <input
                    type="checkbox"
                    checked={info.localOverride}
                    onChange={(ev) => {
                      onOverride(ev.target.checked);
                    }}
                  />
                  Allow local models anyway
                  <span className="text-muted"> — {info.overrideWarning}</span>
                </label>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
}

/** One catalog model: what it needs, its license before Download (US-09.3 AC3), and the
 *  download with progress, pause, resume and cancel (US-09.4). */
export function ModelRowView({
  model,
  busy,
  error,
  onStart,
  onPause,
  onResume,
  onCancel,
  onDelete,
}: {
  model: CatalogModel;
  busy: boolean;
  error: string | null;
  onStart: () => void;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
  onDelete: () => void;
}) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const m = model;
  const running = m.state === "downloading" || m.state === "verifying";
  const partial = m.state === "paused" || m.state === "interrupted";
  return (
    <li className="flex flex-col gap-1 border-b border-border py-2" data-model={m.id}>
      <div className="flex items-baseline gap-3">
        <span className="font-semibold">{m.name}</span>
        {m.recommended && (
          <span className="rounded border border-border px-1.5 text-xs" data-recommended>
            Recommended
          </span>
        )}
        <span className="text-sm text-muted">{roleLabel(m)}</span>
        <span className="ml-auto flex shrink-0 gap-2">
          {m.state === "available" && (
            <button type="button" className={button} disabled={busy} onClick={onStart}>
              Download
            </button>
          )}
          {m.state === "downloading" && (
            <button type="button" className={button} disabled={busy} onClick={onPause}>
              Pause
            </button>
          )}
          {partial && (
            <button type="button" className={button} disabled={busy} onClick={onResume}>
              Resume
            </button>
          )}
          {(m.state === "downloading" || partial) && (
            <button type="button" className={button} disabled={busy} onClick={onCancel}>
              Cancel
            </button>
          )}
          {m.state === "installed" &&
            (confirmDelete ? (
              <>
                <button
                  type="button"
                  className={button}
                  onClick={() => {
                    setConfirmDelete(false);
                  }}
                >
                  Keep
                </button>
                <button
                  type="button"
                  className={button}
                  disabled={busy}
                  onClick={() => {
                    setConfirmDelete(false);
                    onDelete();
                  }}
                >
                  Delete {size(m.bytes)}
                </button>
              </>
            ) : (
              <button
                type="button"
                className={button}
                onClick={() => {
                  setConfirmDelete(true);
                }}
              >
                Delete
              </button>
            ))}
        </span>
      </div>
      <span className="text-sm text-muted">
        {size(m.bytes)} on disk · needs {gb(m.vramBytes)} GB VRAM and {gb(m.ramBytes)} GB RAM
        {!m.fits && " · more than this machine has"} · {m.quant}
      </span>
      {m.diskNeeded !== null && (
        <span className="text-sm" data-disk-needed>
          Does not fit: needs {gb(m.diskNeeded)} GB free disk
        </span>
      )}
      <span className="text-sm" data-license>
        License: {m.license} ·{" "}
        <button
          type="button"
          className="underline"
          title="Opens in your browser"
          onClick={() => {
            void commands.openLink(m.licenseUrl);
          }}
        >
          {m.licenseUrl}
        </button>
      </span>
      {(running || partial) && (
        <div className="flex max-w-2xl flex-col gap-0.5" data-progress>
          <progress
            className="w-full"
            max={100}
            value={percent(m)}
            aria-label={`${m.name} download`}
          />
          <span className="text-sm">
            {stateLabel(m)} · {size(m.bytesDone)} of {size(m.bytes)} ({percent(m)} %)
            {m.state === "downloading" && m.speedBps > 0 && ` · ${speed(m.speedBps)}`}
            {m.state === "downloading" && m.etaS !== null && ` · ${timeLeft(m.etaS)} left`}
          </span>
        </div>
      )}
      {m.state === "installed" && <span className="text-sm">{stateLabel(m)}</span>}
      {m.error !== null && (
        <span role="alert" className="text-sm">
          {m.error.message}
        </span>
      )}
      {error !== null && (
        <span role="alert" className="text-sm">
          {error}
        </span>
      )}
    </li>
  );
}

/** "Models" (US-09.3): the shipped catalog. Not offered below the minimum without the
 *  override (US-09.2 AC1, INV-MM-03). */
export function CatalogSection({
  hardware,
  models,
  busy,
  errors,
  onAction,
}: {
  hardware: HardwareInfo | undefined;
  models: CatalogModel[];
  busy: string | null;
  errors: Record<string, string>;
  onAction: (action: ModelAction, id: string) => void;
}) {
  const allowed = hardware?.localAllowed ?? false;
  return (
    <section aria-label="Models" className="mt-6 flex flex-col gap-2">
      <h3 className="font-semibold">Models</h3>
      {hardware !== undefined && !allowed ? (
        <p className="max-w-2xl text-sm text-muted" data-not-offered>
          Local models are not offered on this machine. Use an endpoint below.
        </p>
      ) : (
        <>
          <p className="max-w-2xl text-sm text-muted">
            Downloaded from Hugging Face and checked against the catalog shipped with this version.
            Recommended models fit this machine best.
          </p>
          <ul className="max-w-4xl">
            {models.map((m) => (
              <ModelRowView
                key={m.id}
                model={m}
                busy={busy === m.id}
                error={errors[m.id] ?? null}
                onStart={() => {
                  onAction("start", m.id);
                }}
                onPause={() => {
                  onAction("pause", m.id);
                }}
                onResume={() => {
                  onAction("resume", m.id);
                }}
                onCancel={() => {
                  onAction("cancel", m.id);
                }}
                onDelete={() => {
                  onAction("delete", m.id);
                }}
              />
            ))}
          </ul>
        </>
      )}
    </section>
  );
}

export type ModelAction = "start" | "pause" | "resume" | "cancel" | "delete";

const run: Record<ModelAction, (id: string) => Promise<null>> = {
  start: commands.modelDownloadStart,
  pause: commands.modelDownloadPause,
  resume: commands.modelDownloadResume,
  cancel: commands.modelDownloadCancel,
  delete: commands.modelDelete,
};

/** The two local-model sections of the AI models screen, with their queries. Both re-query on
 *  `modelsChanged` events. */
export function LocalModels() {
  const client = useQueryClient();
  const hardware = useQuery({
    queryKey: ["hardware"],
    queryFn: () => commands.hardwareCheck(false),
  });
  const catalog = useQuery({ queryKey: ["model_catalog"], queryFn: commands.modelCatalog });
  const [errors, setErrors] = useState<Record<string, string>>({});
  const changed = () => {
    void client.invalidateQueries({ queryKey: ["hardware"] });
    void client.invalidateQueries({ queryKey: ["model_catalog"] });
  };
  const recheck = useMutation({
    mutationFn: () => commands.hardwareCheck(true),
    onSuccess: (info) => {
      client.setQueryData(["hardware"], info);
      void client.invalidateQueries({ queryKey: ["model_catalog"] });
    },
  });
  const override = useMutation({
    mutationFn: commands.localModelsOverride,
    onSuccess: (info) => {
      client.setQueryData(["hardware"], info);
      void client.invalidateQueries({ queryKey: ["model_catalog"] });
    },
  });
  const act = useMutation({
    mutationFn: ({ action, id }: { action: ModelAction; id: string }) => run[action](id),
    onMutate: ({ id }) => {
      setErrors((r) => Object.fromEntries(Object.entries(r).filter(([k]) => k !== id)));
    },
    onError: (e, { id }) => {
      setErrors((r) => ({ ...r, [id]: errorMessage(e) }));
    },
    onSettled: changed,
  });
  const error = hardware.error ?? recheck.error ?? override.error;
  return (
    <>
      <HardwareSection
        info={hardware.data}
        checking={hardware.isFetching || recheck.isPending}
        error={error ? errorMessage(error) : null}
        onCheck={() => {
          recheck.mutate();
        }}
        onOverride={(on) => {
          override.mutate(on);
        }}
      />
      <CatalogSection
        hardware={hardware.data}
        models={catalog.data ?? []}
        busy={act.isPending ? act.variables.id : null}
        errors={errors}
        onAction={(action, id) => {
          act.mutate({ action, id });
        }}
      />
    </>
  );
}

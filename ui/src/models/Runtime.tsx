import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { AiChoice } from "../ipc/bindings/AiChoice";
import type { AiChoiceInfo } from "../ipc/bindings/AiChoiceInfo";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { EndpointItem } from "../ipc/bindings/EndpointItem";
import type { ModelRole } from "../ipc/bindings/ModelRole";
import type { RuntimeInfo } from "../ipc/bindings/RuntimeInfo";
import type { RuntimeModel } from "../ipc/bindings/RuntimeModel";
import type { TryResult } from "../ipc/bindings/TryResult";
import { roleName, runtimeStateLabel } from "../lib/ai";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
const input = "rounded border border-border bg-surface px-2 py-1 text-sm";

function sourceLabel(info: RuntimeInfo): string {
  return info.source === "settings" ? "chosen in settings" : "found on PATH";
}

/** One model the runtime started, with "Show details" (the last llama-server output). */
export function RuntimeModelRow({ model: m }: { model: RuntimeModel }) {
  const [details, setDetails] = useState(false);
  return (
    <li className="flex flex-col gap-0.5 text-sm" data-runtime-model={m.modelId}>
      <span>
        {roleName(m.role)} · {m.modelId} · {runtimeStateLabel(m.state)}
        {m.port !== null && ` · 127.0.0.1:${String(m.port)}`}
        {m.restarts > 0 && ` · restarted ${String(m.restarts)}×`}
      </span>
      {m.failure !== null && <span role="alert">{m.failure.message}</span>}
      {m.tail !== "" && (
        <>
          <button
            type="button"
            className="self-start underline"
            aria-expanded={details}
            onClick={() => {
              setDetails(!details);
            }}
          >
            {details ? "Hide details" : "Show details"}
          </button>
          {details && (
            <pre className="max-h-48 max-w-4xl overflow-auto rounded border border-border p-2 text-xs">
              {m.tail}
            </pre>
          )}
        </>
      )}
    </li>
  );
}

/** "llama-server" (ADR-0015): where it is, its build and backend, or why it cannot be used. */
export function RuntimeSectionView({
  info,
  busy,
  error,
  onChoose,
  onClear,
  onCheck,
  onRetry,
}: {
  info: RuntimeInfo | undefined;
  busy: boolean;
  error: string | null;
  onChoose: () => void;
  onClear: () => void;
  onCheck: () => void;
  onRetry: () => void;
}) {
  const failed = info?.models.some((m) => m.state === "failed") ?? false;
  return (
    <section aria-label="llama-server" className="mt-6 flex flex-col gap-2">
      <div className="flex items-baseline gap-3">
        <h3 className="font-semibold">llama-server</h3>
        <button type="button" className={button} disabled={busy} onClick={onChoose}>
          Choose…
        </button>
        {info?.settingsPath !== null && info?.settingsPath !== undefined && (
          <button type="button" className={button} disabled={busy} onClick={onClear}>
            Use PATH
          </button>
        )}
        <button type="button" className={button} disabled={busy} onClick={onCheck}>
          {busy ? "Checking…" : "Check again"}
        </button>
        {failed && (
          <button type="button" className={button} disabled={busy} onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
      <p className="max-w-2xl text-sm text-muted">
        Local models run in llama.cpp&apos;s llama-server, which you install yourself (build{" "}
        {info?.minBuild ?? 10863} or newer). It runs only while AI work needs it, on this computer
        only.
      </p>
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      {info !== undefined && (
        <>
          <dl className="grid max-w-3xl grid-cols-[8rem_1fr] gap-x-3 gap-y-0.5 text-sm">
            <dt className="text-muted">Program</dt>
            <dd className="font-mono text-xs" data-runtime-path>
              {info.path !== null ? `${info.path} (${sourceLabel(info)})` : "Not found"}
            </dd>
            {info.build !== null && (
              <>
                <dt className="text-muted">Build</dt>
                <dd>
                  {info.build}
                  {info.commit !== null && ` (commit ${info.commit})`}
                </dd>
              </>
            )}
            {info.backends.length > 0 && (
              <>
                <dt className="text-muted">Backend</dt>
                <dd data-backend>{info.backends.join(", ")}</dd>
              </>
            )}
            {info.devices.map((d) => (
              <div key={d.id} className="contents">
                <dt className="text-muted">{d.id}</dt>
                <dd>{d.description}</dd>
              </div>
            ))}
          </dl>
          {info.error !== null && (
            <p role="alert" className="max-w-2xl text-sm" data-runtime-error={info.error.code}>
              {info.error.message}. See {info.installGuide}.
            </p>
          )}
          {info.backendNote !== null && (
            <p role="note" className="max-w-2xl text-sm">
              {info.backendNote}
            </p>
          )}
          {info.models.length > 0 && (
            <ul className="flex flex-col gap-1">
              {info.models.map((m) => (
                <RuntimeModelRow key={m.role} model={m} />
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}

/** The llama-server section with its queries. */
export function RuntimeSection() {
  const client = useQueryClient();
  const status = useQuery({ queryKey: ["runtime"], queryFn: () => commands.runtimeStatus(false) });
  const done = (info: RuntimeInfo) => {
    client.setQueryData(["runtime"], info);
    void client.invalidateQueries({ queryKey: ["ai_readiness"] });
  };
  const choose = useMutation({
    mutationFn: async () => {
      const [path] = await commands.pickFiles("Choose llama-server");
      return path === undefined ? null : commands.runtimeSetPath(path);
    },
    onSuccess: (info) => {
      if (info !== null) done(info);
    },
  });
  const clear = useMutation({ mutationFn: () => commands.runtimeSetPath(null), onSuccess: done });
  const check = useMutation({ mutationFn: () => commands.runtimeStatus(true), onSuccess: done });
  const retry = useMutation({ mutationFn: commands.runtimeRetry, onSuccess: done });
  const busy =
    status.isFetching || choose.isPending || clear.isPending || check.isPending || retry.isPending;
  const error = status.error ?? choose.error ?? clear.error ?? check.error ?? retry.error;
  return (
    <RuntimeSectionView
      info={status.data}
      busy={busy}
      error={error ? errorMessage(error) : null}
      onChoose={() => {
        choose.mutate();
      }}
      onClear={() => {
        clear.mutate();
      }}
      onCheck={() => {
        check.mutate();
      }}
      onRetry={() => {
        retry.mutate();
      }}
    />
  );
}

/** A choice as one `<option>` value. */
function optionValue(c: AiChoice | null): string {
  return c === null ? "" : `${c.kind}:${c.id}`;
}

function parseOption(v: string): AiChoice | null {
  const i = v.indexOf(":");
  if (i < 0) return null;
  const kind = v.slice(0, i);
  const id = v.slice(i + 1);
  return kind === "local" || kind === "endpoint" ? { kind, id } : null;
}

/** "Used for AI": the model or endpoint each role uses (app-level, INV-MM-04). */
export function AiChoiceSectionView({
  choices,
  models,
  endpoints,
  busy,
  error,
  onSet,
}: {
  choices: AiChoiceInfo[];
  models: CatalogModel[];
  endpoints: EndpointItem[];
  busy: boolean;
  error: string | null;
  onSet: (role: ModelRole, choice: AiChoice | null) => void;
}) {
  return (
    <section aria-label="Used for AI" className="mt-4 flex flex-col gap-2">
      <h3 className="font-semibold">Used for AI</h3>
      <p className="max-w-2xl text-sm text-muted">
        What each kind of AI work uses, in every project. An endpoint on another computer or the
        internet also needs to be allowed in each project&apos;s settings.
      </p>
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      <div className="grid max-w-2xl grid-cols-[14rem_1fr] items-center gap-x-3 gap-y-1 text-sm">
        {choices.map((c) => {
          const installed = models.filter((m) => m.role === c.role && m.state === "installed");
          return (
            <label key={c.role} className="contents">
              <span>{roleName(c.role)}</span>
              <select
                className={input}
                disabled={busy}
                value={c.explicit ? optionValue(c.choice) : ""}
                data-choice={c.role}
                onChange={(ev) => {
                  onSet(c.role, parseOption(ev.target.value));
                }}
              >
                <option value="">
                  {c.choice !== null && !c.explicit
                    ? `Default: ${c.name ?? c.choice.id}`
                    : "None: no model"}
                </option>
                {installed.map((m) => (
                  <option key={m.id} value={`local:${m.id}`}>
                    {m.name} (this computer)
                  </option>
                ))}
                {endpoints.map((e) => (
                  <option key={e.id} value={`endpoint:${e.id}`}>
                    {e.name} (endpoint)
                  </option>
                ))}
              </select>
            </label>
          );
        })}
      </div>
    </section>
  );
}

/** "Used for AI" with its queries. */
export function AiChoiceSection() {
  const client = useQueryClient();
  const choices = useQuery({ queryKey: ["ai_choice"], queryFn: commands.aiChoice });
  const catalog = useQuery({ queryKey: ["model_catalog"], queryFn: commands.modelCatalog });
  const endpoints = useQuery({ queryKey: ["endpoints"], queryFn: commands.endpointsList });
  const set = useMutation({
    mutationFn: ({ role, choice }: { role: ModelRole; choice: AiChoice | null }) =>
      commands.aiChoiceSet(role, choice),
    onSuccess: (list) => {
      client.setQueryData(["ai_choice"], list);
      void client.invalidateQueries({ queryKey: ["ai_readiness"] });
      void client.invalidateQueries({ queryKey: ["runtime"] });
    },
  });
  const error = choices.error ?? set.error;
  return (
    <AiChoiceSectionView
      choices={choices.data ?? []}
      models={catalog.data ?? []}
      endpoints={endpoints.data ?? []}
      busy={set.isPending}
      error={error ? errorMessage(error) : null}
      onSet={(role, choice) => {
        set.mutate({ role, choice });
      }}
    />
  );
}

/** What Try found (US-09.5 AC1). */
export function TryResultView({ result: r }: { result: TryResult }) {
  return (
    <span role="status" className="text-sm" data-try={r.ok ? "ok" : "failed"}>
      {r.ok ? "" : "Try failed: "}
      {r.summary}
      {r.bound !== null && ` · ran on ${r.bound}, started in ${(r.startMs / 1000).toFixed(1)} s`}
      {r.structuredOutput === false && " · no structured output"}
    </span>
  );
}

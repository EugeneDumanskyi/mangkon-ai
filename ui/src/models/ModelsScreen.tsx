import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { Destination } from "../ipc/bindings/Destination";
import type { EndpointItem } from "../ipc/bindings/EndpointItem";
import type { EndpointSave } from "../ipc/bindings/EndpointSave";
import type { EndpointTestReport } from "../ipc/bindings/EndpointTestReport";
import type { SecretStoreInfo } from "../ipc/bindings/SecretStoreInfo";
import {
  NO_STRUCTURED_OUTPUT,
  classLabel,
  destinationLine,
  keyInput,
  shownTime,
} from "../lib/egress";
import { LocalModels } from "./LocalModels";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
const input = "rounded border border-border bg-surface px-2 py-1 text-sm";

/** Shown when there is no system keychain (ADR-0012). */
export function SessionNotice({ info }: { info: SecretStoreInfo }) {
  if (info.keyring || info.notice === null) return null;
  return (
    <p role="note" className="max-w-2xl rounded border border-border bg-surface-alt px-3 py-2">
      {info.notice}
    </p>
  );
}

/** What a test found (US-09.6 AC2/AC3). */
export function TestReportView({ report }: { report: EndpointTestReport }) {
  return (
    <div role="status" className="flex flex-col gap-0.5 text-sm" data-test-report>
      <span className={report.ok ? "" : "font-semibold"}>{report.summary}</span>
      {report.ok && (
        <>
          <span className="text-muted">
            {report.origin}
            {report.class !== null && ` · ${classLabel(report.class)}`}
            {report.addresses.length > 0 && ` · ${report.addresses.join(", ")}`}
          </span>
          <span className="text-muted">
            Embeddings:{" "}
            {report.embeddingsDimensions !== null
              ? `${String(report.embeddingsDimensions)} dimensions`
              : `not available${report.embeddingsError !== null ? ` (${report.embeddingsError})` : ""}`}
          </span>
        </>
      )}
    </div>
  );
}

function keyLabel(e: EndpointItem): string {
  switch (e.key) {
    case "none":
      return "No API key";
    case "stored":
      return "API key in the keychain";
    case "missing":
      return "API key missing: enter it again";
  }
}

/** One endpoint on the AI models screen. Never shows the key, only whether there is one. */
export function EndpointRowView({
  endpoint,
  testing,
  report,
  onTest,
  onEdit,
  onDelete,
}: {
  endpoint: EndpointItem;
  testing: boolean;
  report: EndpointTestReport | null;
  onTest: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const e = endpoint;
  return (
    <li className="flex flex-col gap-1 border-b border-border py-2" data-endpoint={e.id}>
      <div className="flex items-baseline gap-3">
        <span className="font-semibold">{e.name}</span>
        <span className="truncate font-mono text-sm">{e.baseUrl}</span>
        <span className="text-sm">{e.model}</span>
        <span className="ml-auto flex shrink-0 gap-2">
          <button type="button" className={button} disabled={testing} onClick={onTest}>
            {testing ? "Testing…" : "Test"}
          </button>
          <button type="button" className={button} onClick={onEdit}>
            Edit
          </button>
          {confirmDelete ? (
            <>
              <button
                type="button"
                className={button}
                onClick={() => {
                  setConfirmDelete(false);
                }}
              >
                Cancel
              </button>
              <button type="button" className={button} onClick={onDelete}>
                Delete endpoint and key
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
          )}
        </span>
      </div>
      <span className="text-sm text-muted">
        {e.destination !== null
          ? destinationLine(e.destination)
          : `Cannot resolve now: ${e.destinationError ?? "unknown"}`}{" "}
        · {keyLabel(e)}
      </span>
      {e.structuredOutput === false && (
        <span role="note" className="text-sm">
          {NO_STRUCTURED_OUTPUT}
        </span>
      )}
      {report !== null ? (
        <TestReportView report={report} />
      ) : (
        e.lastTest !== null && (
          <span className="text-sm text-muted">
            Last test {shownTime(e.lastTest.testedAt)}: {e.lastTest.message}
          </span>
        )
      )}
    </li>
  );
}

/**
 * Add or edit an endpoint (US-09.6 AC1). The key field is a password field held only in this
 * form's state; it is sent once on save and gone when the form closes.
 */
export function EndpointForm({
  editing,
  destination,
  destinationError,
  saving,
  error,
  onCheck,
  onSave,
  onCancel,
}: {
  editing: EndpointItem | null;
  destination: Destination | null;
  destinationError: string | null;
  saving: boolean;
  error: string | null;
  onCheck: (baseUrl: string) => void;
  onSave: (e: EndpointSave) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(editing?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(editing?.baseUrl ?? "");
  const [model, setModel] = useState(editing?.model ?? "");
  const [key, setKey] = useState("");
  const [removeKey, setRemoveKey] = useState(false);
  const hasKey = editing !== null && editing.key !== "none";
  return (
    <form
      aria-label={editing ? "Edit endpoint" : "Add endpoint"}
      className="flex max-w-2xl flex-col gap-3 rounded border border-border p-3"
      autoComplete="off"
      onSubmit={(ev) => {
        ev.preventDefault();
        onSave({
          id: editing?.id ?? null,
          name,
          baseUrl,
          model,
          key: keyInput(key, removeKey, editing !== null),
        });
        setKey("");
      }}
    >
      <label className="flex flex-col gap-1">
        <span className="font-semibold">Base URL</span>
        <span className="text-sm text-muted">
          For example http://192.168.1.20:8080 or https://api.example.com/v1. /v1 is added when the
          URL has no path.
        </span>
        <input
          className={input}
          value={baseUrl}
          required
          spellCheck={false}
          onChange={(ev) => {
            setBaseUrl(ev.target.value);
          }}
          onBlur={() => {
            if (baseUrl.trim() !== "") onCheck(baseUrl);
          }}
        />
      </label>
      {destination !== null && (
        <p className="text-sm" data-destination>
          Goes to {destinationLine(destination)}
        </p>
      )}
      {destinationError !== null && (
        <p role="alert" className="text-sm">
          {destinationError}
        </p>
      )}
      <label className="flex flex-col gap-1">
        <span className="font-semibold">Model</span>
        <input
          className={input}
          value={model}
          required
          spellCheck={false}
          onChange={(ev) => {
            setModel(ev.target.value);
          }}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="font-semibold">Name</span>
        <span className="text-sm text-muted">Optional. The host name is used when empty.</span>
        <input
          className={input}
          value={name}
          onChange={(ev) => {
            setName(ev.target.value);
          }}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="font-semibold">API key</span>
        <span className="text-sm text-muted">
          {hasKey
            ? "A key is stored. Leave empty to keep it, or type a new one."
            : "Optional. Kept in the system keychain, never in a file."}
        </span>
        <input
          className={input}
          type="password"
          value={key}
          autoComplete="new-password"
          spellCheck={false}
          onChange={(ev) => {
            setKey(ev.target.value);
          }}
        />
      </label>
      {hasKey && (
        <label className="flex items-center gap-1 text-sm">
          <input
            type="checkbox"
            checked={removeKey}
            onChange={(ev) => {
              setRemoveKey(ev.target.checked);
            }}
          />
          Remove the stored key
        </label>
      )}
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <button type="submit" className={button} disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </button>
        <button type="button" className={button} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** The app-level AI models screen (E09): this machine and the model catalog (M5b), then the
 *  endpoints (M5a). */
export function ModelsScreen({ onClose }: { onClose: () => void }) {
  const client = useQueryClient();
  const endpoints = useQuery({ queryKey: ["endpoints"], queryFn: commands.endpointsList });
  const secrets = useQuery({ queryKey: ["secret_store"], queryFn: commands.secretStoreStatus });
  // `undefined`: no form; `null`: adding; an item: editing it.
  const [editing, setEditing] = useState<EndpointItem | null | undefined>(undefined);
  const [destination, setDestination] = useState<Destination | null>(null);
  const [destinationError, setDestinationError] = useState<string | null>(null);
  const [reports, setReports] = useState<Record<string, EndpointTestReport>>({});
  const changed = () => {
    void client.invalidateQueries({ queryKey: ["endpoints"] });
    void client.invalidateQueries({ queryKey: ["project_endpoints"] });
  };
  const closeForm = () => {
    setEditing(undefined);
    setDestination(null);
    setDestinationError(null);
  };
  const save = useMutation({
    mutationFn: commands.endpointSave,
    onSuccess: () => {
      closeForm();
      changed();
    },
  });
  const remove = useMutation({ mutationFn: commands.endpointDelete, onSuccess: changed });
  const test = useMutation({
    mutationFn: commands.endpointTest,
    onSuccess: (report, id) => {
      setReports((r) => ({ ...r, [id]: report }));
      changed();
    },
  });
  const check = (baseUrl: string) => {
    commands.resolveDestination(baseUrl).then(
      (d) => {
        setDestination(d);
        setDestinationError(null);
      },
      (e: unknown) => {
        setDestination(null);
        setDestinationError(errorMessage(e));
      },
    );
  };
  const error = endpoints.error ?? remove.error ?? test.error;
  return (
    <div className="h-full overflow-auto p-4">
      <div className="flex items-baseline gap-3">
        <h2 className="text-lg font-semibold">AI models</h2>
        <button type="button" className={`${button} ml-auto`} onClick={onClose}>
          Close
        </button>
      </div>
      <LocalModels />
      <section id="endpoints" aria-label="Endpoints" className="mt-6 flex flex-col gap-2">
        <h3 className="font-semibold">Endpoints</h3>
        <p className="max-w-2xl text-sm text-muted">
          OpenAI-compatible servers, such as llama-server, Ollama or LM Studio on this or another
          computer, or a provider on the internet. No project sends anything to an endpoint until
          you allow it in that project&apos;s settings.
        </p>
        {secrets.data && <SessionNotice info={secrets.data} />}
        {error && (
          <p role="alert" className="text-sm">
            {errorMessage(error)}
          </p>
        )}
        <ul className="max-w-4xl">
          {(endpoints.data ?? []).map((e) => (
            <EndpointRowView
              key={e.id}
              endpoint={e}
              testing={test.isPending && test.variables === e.id}
              report={reports[e.id] ?? null}
              onTest={() => {
                test.mutate(e.id);
              }}
              onEdit={() => {
                save.reset();
                setEditing(e);
                setDestination(e.destination);
                setDestinationError(null);
              }}
              onDelete={() => {
                remove.mutate(e.id);
              }}
            />
          ))}
        </ul>
        {endpoints.data?.length === 0 && <p className="text-sm text-muted">No endpoints yet.</p>}
        {editing === undefined ? (
          <div>
            <button
              type="button"
              className={button}
              onClick={() => {
                save.reset();
                setEditing(null);
              }}
            >
              Add endpoint
            </button>
          </div>
        ) : (
          <EndpointForm
            key={editing?.id ?? ""}
            editing={editing}
            destination={destination}
            destinationError={destinationError}
            saving={save.isPending}
            error={save.error ? errorMessage(save.error) : null}
            onCheck={check}
            onSave={(e) => {
              save.mutate(e);
            }}
            onCancel={closeForm}
          />
        )}
      </section>
    </div>
  );
}

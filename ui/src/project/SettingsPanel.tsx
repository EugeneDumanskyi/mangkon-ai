import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { Settings } from "../ipc/bindings/Settings";
import type { SettingsSaved } from "../ipc/bindings/SettingsSaved";
import type { SourceRoot } from "../ipc/bindings/SourceRoot";
import { CustomPatterns } from "./CustomPatterns";
import { ocrChoices, parseDomains, reprocessQuestion } from "../lib/forms";
import { plural } from "../lib/labels";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
const input = "rounded border border-border bg-surface px-2 py-1 text-sm";

/** The settings form (US-01.6 AC1–AC2). */
export function SettingsForm({
  settings,
  installed,
  saving,
  onSave,
}: {
  settings: Settings;
  installed: string[];
  saving: boolean;
  onSave: (s: Settings) => void;
}) {
  const [ocr, setOcr] = useState(settings.ocrLanguages);
  const [region, setRegion] = useState(settings.phoneRegion ?? "");
  const [domains, setDomains] = useState(settings.domainStopList.join("\n"));
  return (
    <form
      className="flex max-w-2xl flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        onSave({
          ocrLanguages: ocr,
          phoneRegion: region.trim() === "" ? null : region.trim(),
          domainStopList: parseDomains(domains),
        });
      }}
    >
      <fieldset className="flex flex-col gap-1">
        <legend className="font-semibold">OCR languages</legend>
        <p className="text-sm text-muted">
          The languages of scanned pages and images. At least one. Other languages can be added to
          the OCR language folder.
        </p>
        <div className="flex flex-wrap gap-3">
          {ocrChoices(installed, ocr).map((code) => (
            <label key={code} className="flex items-center gap-1 text-sm">
              <input
                type="checkbox"
                checked={ocr.includes(code)}
                onChange={(e) => {
                  setOcr(e.target.checked ? [...ocr, code] : ocr.filter((c) => c !== code));
                }}
              />
              <span className="font-mono">{code}</span>
              {!installed.includes(code) && <span className="text-muted">(not installed)</span>}
            </label>
          ))}
        </div>
      </fieldset>
      <label className="flex flex-col gap-1">
        <span className="font-semibold">Phone region</span>
        <span className="text-sm text-muted">
          Two letters, such as DE, for phone numbers written without a country code. Left empty,
          such numbers are kept as written digits and only match the same digits.
        </span>
        <input
          className={`${input} w-24 uppercase`}
          value={region}
          maxLength={2}
          onChange={(e) => {
            setRegion(e.target.value);
          }}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="font-semibold">Domains that are not connections</span>
        <span className="text-sm text-muted">
          One per line. A domain on this list, such as a webmail domain, is never shown as a
          connection. Changes apply at once.
        </span>
        <textarea
          className={`${input} h-32 font-mono`}
          value={domains}
          onChange={(e) => {
            setDomains(e.target.value);
          }}
        />
      </label>
      <div>
        <button
          type="submit"
          className="rounded bg-accent px-3 py-1 text-sm text-accent-text disabled:opacity-50"
          disabled={saving || ocr.length === 0}
        >
          Save
        </button>
      </div>
    </form>
  );
}

/** One source folder or file, with Locate… when it is gone and Remove (D12, US-01.5). */
export function SourceRootRow({
  root,
  busy,
  onLocate,
  onRemove,
}: {
  root: SourceRoot;
  busy: boolean;
  onLocate: () => void;
  onRemove: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  return (
    <li className="flex flex-wrap items-center gap-2 border-b border-border py-1.5 text-sm">
      <span className="min-w-0 flex-1 truncate font-mono" title={root.path}>
        {root.path}
      </span>
      <span className="text-muted">
        {root.kind}
        {!root.present && " · not found"}
        {root.excluded > 0 && ` · ${plural(root.excluded, "path")} removed`}
      </span>
      <button type="button" className={button} disabled={busy} onClick={onLocate}>
        Locate…
      </button>
      {confirming ? (
        <>
          <span>Remove this source and every file in it from the project?</span>
          <button type="button" className={button} disabled={busy} onClick={onRemove}>
            Remove
          </button>
          <button
            type="button"
            className={button}
            onClick={() => {
              setConfirming(false);
            }}
          >
            Keep
          </button>
        </>
      ) : (
        <button
          type="button"
          className={button}
          disabled={busy}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Remove
        </button>
      )}
    </li>
  );
}

/** "Re-process affected files now?" after a settings change (US-01.6 AC3, US-04.2 AC2). */
export function ReprocessPrompt({
  saved,
  busy,
  onReprocess,
  onLater,
}: {
  saved: SettingsSaved;
  busy: boolean;
  onReprocess: () => void;
  onLater: () => void;
}) {
  const question = reprocessQuestion(saved);
  if (question === null) return null;
  return (
    <div role="alertdialog" className="mt-3 flex flex-wrap items-center gap-2 text-sm">
      {question}
      {saved.rerunIdentifiers && (
        <span className="text-muted">
          Phone numbers are found again from the stored text; no file is read again.
        </span>
      )}
      <button type="button" className={button} disabled={busy} onClick={onReprocess}>
        Re-process
      </button>
      <button type="button" className={button} onClick={onLater}>
        Later
      </button>
    </div>
  );
}

/** Project settings and sources (US-01.6, D12). */
export function SettingsPanel() {
  const client = useQueryClient();
  const info = useQuery({ queryKey: ["settings"], queryFn: commands.projectSettings });
  const roots = useQuery({ queryKey: ["roots"], queryFn: commands.sourceRoots });
  const [askReprocess, setAskReprocess] = useState<SettingsSaved | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: commands.saveSettings,
    onSuccess: (r) => {
      const ask = reprocessQuestion(r) !== null;
      setAskReprocess(ask ? r : null);
      setSaved(ask ? null : "Saved.");
      void client.invalidateQueries({ queryKey: ["settings"] });
      // A stop-list change applies at once.
      void client.invalidateQueries({ queryKey: ["connections"] });
    },
  });
  const reprocess = useMutation({
    mutationFn: (r: SettingsSaved) =>
      commands.reprocess({ ocr: r.reprocessOcr, identifiers: r.rerunIdentifiers }),
    onSuccess: (n, r) => {
      setAskReprocess(null);
      const parts = [
        r.reprocessOcr ? `${plural(n, "file")} will be read again` : null,
        r.rerunIdentifiers ? "phone numbers will be found again" : null,
      ].filter((p) => p !== null);
      setSaved(`Saved. ${parts.join("; ")}.`);
      void client.invalidateQueries({ queryKey: ["progress"] });
    },
  });
  const refreshRoots = () => {
    void client.invalidateQueries({ queryKey: ["roots"] });
    void client.invalidateQueries({ queryKey: ["project"] });
    void client.invalidateQueries({ queryKey: ["documents"] });
  };
  const locate = useMutation({
    mutationFn: async (root: SourceRoot) => {
      const path =
        root.kind === "file"
          ? ((await commands.pickFiles(`Locate ${root.path}`))[0] ?? null)
          : await commands.pickFolder(`Locate ${root.path}`);
      if (path !== null) await commands.relocateRoot(root.id, path);
    },
    onSuccess: refreshRoots,
  });
  const removeRoot = useMutation({
    mutationFn: (root: SourceRoot) => commands.removeRoot(root.id),
    onSuccess: refreshRoots,
  });
  const error = info.error ?? roots.error ?? save.error ?? reprocess.error;
  const rootError = locate.error ?? removeRoot.error;

  return (
    <div className="h-full overflow-auto p-4">
      {error && (
        <p role="alert" className="mb-3 text-sm">
          {errorMessage(error)}
        </p>
      )}
      {info.data && (
        <SettingsForm
          // A fresh form after each save, from what was stored.
          key={JSON.stringify(info.data.settings)}
          settings={info.data.settings}
          installed={info.data.installedOcrLanguages}
          saving={save.isPending}
          onSave={(s) => {
            setSaved(null);
            save.mutate(s);
          }}
        />
      )}
      {askReprocess !== null && (
        <ReprocessPrompt
          saved={askReprocess}
          busy={reprocess.isPending}
          onReprocess={() => {
            reprocess.mutate(askReprocess);
          }}
          onLater={() => {
            setAskReprocess(null);
            setSaved("Saved. Files read from now on use the new settings.");
          }}
        />
      )}
      {saved !== null && (
        <p role="status" className="mt-3 text-sm">
          {saved}
        </p>
      )}
      <CustomPatterns />
      <h3 className="mt-6 font-semibold">Sources</h3>
      {rootError && (
        <p role="alert" className="text-sm">
          {errorMessage(rootError)}
        </p>
      )}
      <ul className="max-w-3xl">
        {(roots.data ?? []).map((r) => (
          <SourceRootRow
            key={r.id}
            root={r}
            busy={locate.isPending || removeRoot.isPending}
            onLocate={() => {
              locate.mutate(r);
            }}
            onRemove={() => {
              removeRoot.mutate(r);
            }}
          />
        ))}
      </ul>
      {roots.data?.length === 0 && <p className="text-sm text-muted">No sources yet.</p>}
    </div>
  );
}

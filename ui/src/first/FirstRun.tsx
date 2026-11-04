import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { CatalogModel } from "../ipc/bindings/CatalogModel";
import type { Destination } from "../ipc/bindings/Destination";
import type { EndpointSave } from "../ipc/bindings/EndpointSave";
import type { HardwareInfo } from "../ipc/bindings/HardwareInfo";
import type { ProjectInfo } from "../ipc/bindings/ProjectInfo";
import { HardwareSection, type ModelAction, ModelRowView } from "../models/LocalModels";
import { EndpointForm } from "../models/ModelsScreen";
import { NewProjectForm } from "../project/StartScreen";

const button = "rounded border border-border px-3 py-1 text-sm disabled:opacity-50";
const primary = "rounded border border-current px-3 py-1 text-sm font-semibold disabled:opacity-50";

export type Step = 1 | 2 | 3;

const STEPS: [Step, string][] = [
  [1, "This machine"],
  [2, "AI model"],
  [3, "First project"],
];

/** "Step 2 of 3", with the three step names (US-14.4 AC1). */
export function StepList({ step }: { step: Step }) {
  return (
    <ol aria-label={`Step ${String(step)} of 3`} className="flex gap-4 text-sm">
      {STEPS.map(([n, name]) => (
        <li
          key={n}
          aria-current={n === step ? "step" : undefined}
          className={n === step ? "font-semibold" : "text-muted"}
        >
          {n}. {name}
        </li>
      ))}
    </ol>
  );
}

/** Step 2: download a recommended model, use an endpoint, or go on without AI. Below the minimum
 *  only the endpoint and Skip are offered (US-09.2 AC1). */
export function ChooseModelStep({
  hardware,
  models,
  busy,
  errors,
  endpointSaved,
  onAction,
  onEndpoint,
  onNext,
}: {
  hardware: HardwareInfo | undefined;
  models: CatalogModel[];
  busy: string | null;
  errors: Record<string, string>;
  endpointSaved: string | null;
  onAction: (action: ModelAction, id: string) => void;
  onEndpoint: () => void;
  onNext: () => void;
}) {
  const allowed = hardware?.localAllowed ?? false;
  const recommended = models.filter((m) => m.recommended);
  const ready = models.some((m) => m.state === "installed") || endpointSaved !== null;
  return (
    <section aria-label="AI model" className="flex flex-col gap-3">
      <p className="max-w-2xl text-sm">
        AI finds people and organizations and lets you search by meaning. Everything else works
        without it.
      </p>
      {allowed ? (
        <>
          <p className="max-w-2xl text-sm text-muted">
            Recommended for this machine. Models run on this computer; nothing is sent anywhere.
          </p>
          <ul className="max-w-4xl" data-recommended-models>
            {recommended.map((m) => (
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
          <p className="max-w-2xl text-sm text-muted">
            Local models need llama-server (llama.cpp), which you install yourself. The AI models
            screen shows whether it was found.
          </p>
        </>
      ) : (
        hardware?.tooWeak != null && (
          <p role="note" className="max-w-2xl text-sm" data-too-weak>
            {hardware.tooWeak}
          </p>
        )
      )}
      {endpointSaved !== null && (
        <p role="status" className="text-sm">
          {endpointSaved} is used for people and organizations.
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <button type="button" className={button} onClick={onEndpoint}>
          Use an endpoint…
        </button>
        <button type="button" className={ready ? primary : button} onClick={onNext}>
          {ready ? "Next" : "Skip — use without AI"}
        </button>
      </div>
    </section>
  );
}

/** Step 3: a new project, or the demo project with the test files (US-14.4 AC2). */
export function FirstProjectStep({
  demo,
  busy,
  demoError,
  children,
  onDemo,
}: {
  /** Where the demo files were found, if they were. */
  demo: string | null;
  busy: boolean;
  demoError: string | null;
  /** The New project form. */
  children: React.ReactNode;
  onDemo: () => void;
}) {
  return (
    <section aria-label="First project" className="flex flex-col gap-3">
      {children}
      <div className="flex max-w-md flex-col gap-1 rounded border border-border p-4 text-sm">
        <h3 className="font-semibold">Demo project</h3>
        <p className="text-muted">
          A small set of made-up invoices, emails, tables and scans to try Mangkon with. They come
          with the source code (testdata/td1).
        </p>
        {demo !== null ? (
          <p className="truncate font-mono text-xs" title={demo} data-demo-path>
            {demo}
          </p>
        ) : (
          <p data-demo-missing>Not found next to the app: choose the testdata/td1 folder.</p>
        )}
        <div>
          <button type="button" className={button} disabled={busy} onClick={onDemo}>
            {demo !== null ? "Open demo project" : "Choose the demo folder…"}
          </button>
        </div>
        {demoError !== null && <p role="alert">{demoError}</p>}
      </div>
    </section>
  );
}

const run: Record<ModelAction, (id: string) => Promise<null>> = {
  start: commands.modelDownloadStart,
  pause: commands.modelDownloadPause,
  resume: commands.modelDownloadResume,
  cancel: commands.modelDownloadCancel,
  delete: commands.modelDelete,
};

/** The first-run screen (US-14.4): three steps, each can be skipped. */
export function FirstRun({ demo, onDone }: { demo: string | null; onDone: () => void }) {
  const client = useQueryClient();
  const [step, setStep] = useState<Step>(1);
  const hardware = useQuery({
    queryKey: ["hardware"],
    queryFn: () => commands.hardwareCheck(false),
  });
  const catalog = useQuery({ queryKey: ["model_catalog"], queryFn: commands.modelCatalog });
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [endpointForm, setEndpointForm] = useState(false);
  const [endpointSaved, setEndpointSaved] = useState<string | null>(null);
  const [destination, setDestination] = useState<Destination | null>(null);
  const [destinationError, setDestinationError] = useState<string | null>(null);
  const [location, setLocation] = useState<string | null>(null);

  const finish = async () => {
    await commands.firstRunFinish();
    void client.invalidateQueries({ queryKey: ["first_run"] });
    onDone();
  };
  const opened = (info: ProjectInfo | null) => {
    if (info === null) return;
    client.setQueryData(["project"], info);
    void finish();
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
    onError: (e, { id }) => {
      setErrors((r) => ({ ...r, [id]: errorMessage(e) }));
    },
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["model_catalog"] });
    },
  });
  const saveEndpoint = useMutation({
    mutationFn: async (e: EndpointSave) => {
      const saved = await commands.endpointSave(e);
      await commands.aiChoiceSet("extraction", { kind: "endpoint", id: saved.id });
      return saved;
    },
    onSuccess: (saved) => {
      setEndpointForm(false);
      setEndpointSaved(saved.name);
      void client.invalidateQueries({ queryKey: ["endpoints"] });
      void client.invalidateQueries({ queryKey: ["ai_choice"] });
    },
  });
  const create = useMutation({
    mutationFn: (name: string) => commands.createProject(name, location ?? ""),
    onSuccess: opened,
  });
  const openDemo = useMutation({
    mutationFn: async () => {
      if (demo !== null) return commands.demoOpen(demo);
      const path = await commands.pickFolder("Choose the demo folder (testdata/td1)");
      return path === null ? null : commands.demoOpen(path);
    },
    onSuccess: opened,
  });
  const hwError = hardware.error ?? recheck.error ?? override.error;

  return (
    <div className="flex h-full flex-col gap-4 overflow-auto p-6">
      <div className="flex items-baseline gap-3">
        <h2 className="text-lg font-semibold">Welcome to Mangkon AI</h2>
        <button type="button" className="ml-auto text-sm underline" onClick={() => void finish()}>
          Skip setup
        </button>
      </div>
      <StepList step={step} />
      {step === 1 && (
        <>
          <HardwareSection
            info={hardware.data}
            checking={hardware.isFetching || recheck.isPending}
            error={hwError ? errorMessage(hwError) : null}
            onCheck={() => {
              recheck.mutate();
            }}
            onOverride={(on) => {
              override.mutate(on);
            }}
          />
          <div>
            <button
              type="button"
              className={primary}
              onClick={() => {
                setStep(2);
              }}
            >
              Next
            </button>
          </div>
        </>
      )}
      {step === 2 && (
        <>
          <ChooseModelStep
            hardware={hardware.data}
            models={catalog.data ?? []}
            busy={act.isPending ? act.variables.id : null}
            errors={errors}
            endpointSaved={endpointSaved}
            onAction={(action, id) => {
              act.mutate({ action, id });
            }}
            onEndpoint={() => {
              saveEndpoint.reset();
              setEndpointForm(true);
            }}
            onNext={() => {
              setStep(3);
            }}
          />
          {endpointForm && (
            <EndpointForm
              editing={null}
              destination={destination}
              destinationError={destinationError}
              saving={saveEndpoint.isPending}
              error={saveEndpoint.error ? errorMessage(saveEndpoint.error) : null}
              onCheck={(baseUrl) => {
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
              }}
              onSave={(e) => {
                saveEndpoint.mutate(e);
              }}
              onCancel={() => {
                setEndpointForm(false);
              }}
            />
          )}
        </>
      )}
      {step === 3 && (
        <FirstProjectStep
          demo={demo}
          busy={openDemo.isPending || create.isPending}
          demoError={openDemo.error ? errorMessage(openDemo.error) : null}
          onDemo={() => {
            openDemo.mutate();
          }}
        >
          <NewProjectForm
            location={location}
            pending={create.isPending}
            error={create.error}
            onPickLocation={() => {
              void commands.pickFolder("Choose where to make the project folder").then((p) => {
                if (p !== null) setLocation(p);
              });
            }}
            onCreate={(name) => {
              create.mutate(name);
            }}
            onCancel={() => void finish()}
          />
        </FirstProjectStep>
      )}
    </div>
  );
}

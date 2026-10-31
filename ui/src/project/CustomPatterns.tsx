import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { commands, errorMessage } from "../ipc/commands";
import type { CustomPatternItem } from "../ipc/bindings/CustomPatternItem";
import type { PatternTest } from "../ipc/bindings/PatternTest";
import { plural } from "../lib/labels";

const button = "rounded border border-border px-2 py-0.5 text-sm disabled:opacity-50";
const input = "rounded border border-border bg-surface px-2 py-1 text-sm";

/** How long the test box waits after the last key press (US-04.7 AC1). */
export const TEST_DELAY_MS = 250;

/** The warning on a pattern with more than 10,000 matches (US-04.7 AC4). */
export const TOO_BROAD = "This pattern matches a lot. It may be too broad.";

/** The sample with the matches marked, or why the expression cannot be used (AC1/AC2). */
function TestResult({ test }: { test: PatternTest }) {
  if (test.kind === "error") {
    return (
      <p role="alert" className="text-sm">
        {test.message}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      <p className="text-sm text-muted">{plural(test.count, "match", "matches")}</p>
      <p
        className="whitespace-pre-wrap rounded border border-border p-2 font-mono text-sm"
        aria-label="Sample with matches"
      >
        {test.sample.map((piece, i) =>
          piece.hit ? <mark key={i}>{piece.text}</mark> : <span key={i}>{piece.text}</span>,
        )}
      </p>
    </div>
  );
}

/**
 * Add or edit a pattern: name, expression and a sample box with live matches. Save stays
 * disabled while the expression has an error. A saved pattern keeps its name; renaming is
 * remove and add.
 */
export function PatternEditor({
  editing,
  saving,
  error,
  onSave,
  onCancel,
}: {
  editing: CustomPatternItem | null;
  saving: boolean;
  error: string | null;
  onSave: (name: string, expression: string) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState(editing?.name ?? "");
  const [expression, setExpression] = useState(editing?.expression ?? "");
  const [sample, setSample] = useState("");
  const [test, setTest] = useState<PatternTest | null>(null);
  useEffect(() => {
    if (expression === "") return;
    let live = true;
    const timer = setTimeout(() => {
      void commands.testPattern(expression, sample).then((t) => {
        if (live) setTest(t);
      });
    }, TEST_DELAY_MS);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [expression, sample]);
  // An emptied expression shows no stale result.
  const shown = expression === "" ? null : test;
  const invalid = shown?.kind === "error";
  return (
    <form
      className="flex max-w-2xl flex-col gap-2 rounded border border-border p-3"
      aria-label={editing ? `Edit pattern ${editing.name}` : "Add pattern"}
      onSubmit={(e) => {
        e.preventDefault();
        onSave(name.trim(), expression);
      }}
    >
      <label className="flex flex-col gap-1 text-sm">
        Name
        <input
          className={input}
          value={name}
          disabled={editing !== null}
          maxLength={40}
          onChange={(e) => {
            setName(e.target.value);
          }}
        />
      </label>
      <label className="flex flex-col gap-1 text-sm">
        Pattern
        <input
          className={`${input} font-mono`}
          value={expression}
          spellCheck={false}
          onChange={(e) => {
            setExpression(e.target.value);
          }}
        />
      </label>
      <label className="flex flex-col gap-1 text-sm">
        Test text
        <textarea
          className={`${input} font-mono`}
          rows={3}
          value={sample}
          onChange={(e) => {
            setSample(e.target.value);
          }}
        />
      </label>
      {shown !== null && <TestResult test={shown} />}
      {error !== null && (
        <p role="alert" className="text-sm">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <button
          type="submit"
          className={button}
          disabled={saving || invalid || name.trim() === "" || expression === ""}
        >
          Save
        </button>
        <button type="button" className={button} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/** The "Custom patterns" section of the Settings tab (US-04.7). */
export function CustomPatterns() {
  const client = useQueryClient();
  const patterns = useQuery({ queryKey: ["patterns"], queryFn: commands.customPatterns });
  // `undefined`: no editor open; `null`: adding.
  const [editing, setEditing] = useState<CustomPatternItem | null | undefined>(undefined);
  const changed = () => {
    void client.invalidateQueries({ queryKey: ["patterns"] });
    void client.invalidateQueries({ queryKey: ["progress"] });
    void client.invalidateQueries({ queryKey: ["connections"] });
  };
  const save = useMutation({
    mutationFn: ({ name, expression }: { name: string; expression: string }) =>
      editing ? commands.editPattern(name, expression) : commands.addPattern(name, expression),
    onSuccess: () => {
      setEditing(undefined);
      changed();
    },
  });
  const remove = useMutation({ mutationFn: commands.removePattern, onSuccess: changed });
  const error = patterns.error ?? remove.error;
  return (
    <section aria-label="Custom patterns" className="mt-6 flex flex-col gap-2">
      <h3 className="font-semibold">Custom patterns</h3>
      <p className="max-w-2xl text-sm text-muted">
        Find your own identifiers, such as case or tender numbers. Each pattern becomes an
        identifier type named after it. Saving finds them again in the stored text.
      </p>
      {error && (
        <p role="alert" className="text-sm">
          {errorMessage(error)}
        </p>
      )}
      <ul className="max-w-3xl">
        {(patterns.data ?? []).map((p) => (
          <li key={p.name} className="flex items-baseline gap-3 border-b border-border py-1">
            <span className="font-semibold">{p.name}</span>
            <span className="truncate font-mono text-sm">{p.expression}</span>
            <span className="shrink-0 text-sm text-muted">
              {plural(p.matchCount, "match", "matches")}
            </span>
            {p.tooBroad && (
              <span role="note" className="shrink-0 text-sm">
                {TOO_BROAD}
              </span>
            )}
            <span className="ml-auto flex shrink-0 gap-2">
              <button
                type="button"
                className={button}
                onClick={() => {
                  save.reset();
                  setEditing(p);
                }}
              >
                Edit
              </button>
              <button
                type="button"
                className={button}
                disabled={remove.isPending}
                onClick={() => {
                  remove.mutate(p.name);
                }}
              >
                Remove
              </button>
            </span>
          </li>
        ))}
      </ul>
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
            Add pattern
          </button>
        </div>
      ) : (
        <PatternEditor
          key={editing?.name ?? ""}
          editing={editing}
          saving={save.isPending}
          error={save.error ? errorMessage(save.error) : null}
          onSave={(name, expression) => {
            save.mutate({ name, expression });
          }}
          onCancel={() => {
            setEditing(undefined);
          }}
        />
      )}
    </section>
  );
}

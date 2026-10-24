import { useQuery } from "@tanstack/react-query";
import { commands } from "./ipc/commands";
import { S2dScreen } from "./spike/S2dScreen";

export function App() {
  const info = useQuery({ queryKey: ["app_info"], queryFn: commands.appInfo });

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-baseline gap-3 border-b border-border bg-surface-alt px-4 py-2">
        <h1 className="text-base font-semibold">Mangkon AI</h1>
        {info.data && (
          <span className="text-sm text-muted">
            {info.data.version} · {info.data.ocrEngine}
          </span>
        )}
        {info.error && <span className="text-sm text-muted">Core not reachable</span>}
      </header>
      <main className="min-h-0 flex-1">
        {info.data?.spikeS2d ? (
          <S2dScreen autorun={info.data.spikeS2d.autorun} />
        ) : (
          <p className="p-4 text-muted">No project open. Projects come in a later build.</p>
        )}
      </main>
    </div>
  );
}

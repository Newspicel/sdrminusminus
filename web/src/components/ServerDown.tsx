import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { serverReachable } from "../lib/api";
import { Button } from "./BaseControls";
import { BTN_PRIMARY } from "./controls";
import { serverDownDetail } from "./serverStatus";

const PROBE_MS = 3000;

export function ServerDown({ reason, onReachable }: { reason: string; onReachable: () => void }) {
  const queryClient = useQueryClient();
  const detail = serverDownDetail(reason);

  const reconnect = useCallback(() => {
    onReachable();
    void queryClient.refetchQueries({ type: "active" });
  }, [onReachable, queryClient]);

  useEffect(() => {
    let stopped = false;
    let timer = 0;
    const tick = async () => {
      const reachable = await serverReachable();
      if (stopped) {
        return;
      }
      if (reachable) {
        reconnect();
      }
      timer = window.setTimeout(() => void tick(), PROBE_MS);
    };
    timer = window.setTimeout(() => void tick(), PROBE_MS);
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [reconnect]);

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center bg-bg px-4 py-10">
      <div
        role="alert"
        className="flex w-full max-w-md flex-col gap-3 rounded border border-line bg-panel px-4 py-4"
      >
        <div className="flex items-center gap-2">
          <img src="/icon.svg" alt="" width={28} height={28} className="shrink-0" />
          <div className="font-mono text-lg font-semibold text-accent">sdr--</div>
        </div>
        <div>
          <h1 className="text-sm font-semibold text-ink">Can't reach the server</h1>
          <p className="mt-1 text-sm text-ink-dim">
            This window is running, but the sdr-- server behind it is not answering. Nothing you
            arranged is lost — it lives on the server, and this page picks it up again by itself as
            soon as the server is back.
          </p>
        </div>
        {detail !== null && (
          <p className="font-mono text-xs break-words text-ink-faint">{detail}</p>
        )}
        {import.meta.env.DEV && (
          <p className="text-sm text-ink-dim">
            Start it with <code className="font-mono text-ink">cargo run -p sdrmm</code> — the dev
            server proxies <code className="font-mono text-ink">/api</code> to 127.0.0.1:8080.
          </p>
        )}
        <div className="flex items-center gap-3">
          <Button type="button" className={BTN_PRIMARY} onClick={reconnect}>
            Try again
          </Button>
          <span className="text-xs text-ink-faint">Retrying every few seconds…</span>
        </div>
      </div>
    </div>
  );
}

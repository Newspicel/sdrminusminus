import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { serverReachable } from "../lib/api";

/** Failures that say nothing the headline has not already said. A browser reports a refused
 * connection as "Failed to fetch" (Safari: "Load failed"), and the dev proxy turns the same
 * refusal into an empty 500 — repeating any of those under "can't reach the server" is noise
 * dressed as a diagnosis. Anything else is the server's own words and worth showing. */
const OPAQUE = [
  /^failed to fetch$/i,
  /^load failed$/i,
  /^networkerror\b/i,
  /^typeerror:/i,
  /no response from the server$/i,
];

export function serverDownDetail(reason: string | null): string | null {
  const trimmed = reason?.trim() ?? "";
  if (trimmed === "" || OPAQUE.some((pattern) => pattern.test(trimmed))) {
    return null;
  }
  return trimmed;
}

/** How often this screen probes. The socket's own backoff climbs to 30 s, which is right for a
 * tab left open overnight and wrong for someone reading this screen while starting the server. */
const PROBE_MS = 3000;

/**
 * Shown when the startup reads did not answer at all: the workspace list is unknown, so "there
 * are no workspaces" is not something we know — offering to create one would be a button whose
 * click can only fail.
 */
export function ServerDown({ reason, onReachable }: { reason: string; onReachable: () => void }) {
  const queryClient = useQueryClient();
  const detail = serverDownDetail(reason);

  const reconnect = useCallback(() => {
    // The socket backs off on its own schedule; bring it along, or the app can come back with a
    // live REST layer and no event stream behind it.
    onReachable();
    void queryClient.refetchQueries({ type: "active" });
  }, [onReachable, queryClient]);

  // Probe one cheap endpoint rather than refetching every startup query: a server that is still
  // down should cost one refused connection per attempt, not five.
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
    <div className="flex min-h-0 flex-1 items-center justify-center bg-background px-4 py-10">
      <Card role="alert" className="w-full max-w-md">
        <CardHeader>
          <div className="flex items-center gap-2">
            <img src="/icon.svg" alt="" width={28} height={28} className="shrink-0" />
            <div className="font-mono text-lg font-semibold text-primary">sdr--</div>
          </div>
          <CardTitle>Can't reach the server</CardTitle>
          <CardDescription>
            This window is running, but the sdr-- server behind it is not answering. Nothing you
            arranged is lost — it lives on the server, and this page picks it up again by itself as
            soon as the server is back.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          {detail !== null && (
            <p className="font-mono text-xs break-words text-muted-foreground/70">{detail}</p>
          )}
          {import.meta.env.DEV && (
            <p className="text-sm text-muted-foreground">
              Start it with <code className="font-mono text-foreground">cargo run -p sdrmm</code> —
              the dev server proxies <code className="font-mono text-foreground">/api</code> to
              127.0.0.1:8080.
            </p>
          )}
        </CardContent>
        <CardFooter className="gap-3">
          <Button type="button" onClick={reconnect}>
            Try again
          </Button>
          <span className="text-xs text-muted-foreground/70">Retrying every few seconds…</span>
        </CardFooter>
      </Card>
    </div>
  );
}

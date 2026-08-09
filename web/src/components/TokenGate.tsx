// Shared-token prompt (PLAN §12). The probe is `GET /api/auth`, which answers without
// authentication — the browser WebSocket API reports a rejected handshake as a plain close,
// indistinguishable from "server down", so without this probe a wrong token would look like
// an outage and reconnect forever.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { authQuery } from "../lib/api";
import { getToken, setToken } from "../lib/auth";
import { BTN, FIELD } from "./controls";

/** Renders `children` once the server is reachable without a prompt, or the prompt itself when
 * a token is required and none is stored. */
export function TokenGate({ onToken, children }: { onToken: () => void; children: ReactNode }) {
  const queryClient = useQueryClient();
  const auth = useQuery(authQuery());
  const [entry, setEntry] = useState("");
  const [saved, setSaved] = useState(getToken() !== null);

  // While the probe is in flight the app renders: the probe is the only unauthenticated call,
  // so a slow one must not blank the UI, and every other request will 401 harmlessly until it
  // resolves.
  if (auth.data?.token_required !== true || saved) {
    return children;
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-bg px-4 py-10">
      <form
        className="flex w-full max-w-sm flex-col gap-3 rounded border border-line bg-panel px-4 py-4"
        onSubmit={(e) => {
          e.preventDefault();
          const token = entry.trim();
          if (token === "") {
            return;
          }
          setToken(token);
          setSaved(true);
          // Everything fetched before the token existed was a 401; start clean.
          void queryClient.invalidateQueries();
          onToken();
        }}
      >
        <div>
          <div className="font-mono text-lg font-semibold text-accent">sdr--</div>
          <p className="mt-1 text-sm text-ink-dim">
            This server requires its shared token (the value it was started with as
            <code className="mx-1 font-mono">--token</code>).
          </p>
        </div>
        <input
          className={FIELD}
          type="password"
          autoComplete="current-password"
          aria-label="Shared token"
          value={entry}
          onChange={(e) => setEntry(e.target.value)}
        />
        <button type="submit" className={BTN} disabled={entry.trim() === ""}>
          Connect
        </button>
      </form>
    </div>
  );
}

/** Forget a token the server has rejected, so the gate asks again instead of the client
 * retrying a credential that will never work. */
export function clearToken(): void {
  setToken(null);
}

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { authQuery } from "../lib/api";
import { getToken, onTokenRejected, setToken } from "../lib/auth";
import { Button, Form, Input } from "./BaseControls";
import { BTN, FIELD } from "./controls";

export function TokenGate({ onToken, children }: { onToken: () => void; children: ReactNode }) {
  const queryClient = useQueryClient();
  const auth = useQuery(authQuery());
  const [entry, setEntry] = useState("");
  const [saved, setSaved] = useState(getToken() !== null);
  const [refused, setRefused] = useState(false);

  useEffect(
    () =>
      onTokenRejected(() => {
        setSaved(false);
        setRefused(true);
      }),
    [],
  );

  if (auth.data?.token_required !== true || saved) {
    return children;
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-bg px-4 py-10">
      <Form
        className="flex w-full max-w-sm flex-col gap-3 rounded border border-line bg-panel px-4 py-4"
        onSubmit={(e) => {
          e.preventDefault();
          const token = entry.trim();
          if (token === "") {
            return;
          }
          setToken(token);
          setSaved(true);
          setRefused(false);
          void queryClient.invalidateQueries();
          onToken();
        }}
      >
        <div>
          <div className="flex items-center gap-2">
            <img src="/icon.svg" alt="" width={28} height={28} className="shrink-0" />
            <div className="font-mono text-lg font-semibold text-accent">sdr--</div>
          </div>
          <p className="mt-1 text-sm text-ink-dim">
            This server requires its shared token (the value it was started with as
            <code className="mx-1 font-mono">--token</code>).
          </p>
          {refused && (
            <p role="alert" className="mt-1 font-mono text-sm text-danger">
              That token was refused.
            </p>
          )}
        </div>
        <Input
          className={`${FIELD} w-full`}
          type="password"
          autoComplete="current-password"
          aria-label="Shared token"
          value={entry}
          onChange={(e) => setEntry(e.target.value)}
        />
        <Button type="submit" className={BTN} disabled={entry.trim() === ""}>
          Connect
        </Button>
      </Form>
    </div>
  );
}

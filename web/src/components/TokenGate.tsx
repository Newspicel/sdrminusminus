import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { authQuery } from "../lib/api";
import { getToken, onTokenRejected, setToken } from "../lib/auth";

/** Renders `children` once the server is reachable without a prompt, or the prompt itself when
 * a token is required and none is stored. */
export function TokenGate({ onToken, children }: { onToken: () => void; children: ReactNode }) {
  const queryClient = useQueryClient();
  const auth = useQuery(authQuery());
  const [entry, setEntry] = useState("");
  const [saved, setSaved] = useState(getToken() !== null);
  const [refused, setRefused] = useState(false);

  // The token the browser had was refused (wrong, or the server's was changed). Come back and
  // ask, rather than leaving every request failing behind a UI that looks fine.
  useEffect(
    () =>
      onTokenRejected(() => {
        setSaved(false);
        setRefused(true);
      }),
    [],
  );

  // While the probe is in flight the app renders: the probe is the only unauthenticated call,
  // so a slow one must not blank the UI, and every other request will 401 harmlessly until it
  // resolves.
  if (auth.data?.token_required !== true || saved) {
    return children;
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-background px-4 py-10">
      <Card className="w-full max-w-sm">
        <form
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
          <CardHeader>
            <div className="flex items-center gap-2">
              <img src="/icon.svg" alt="" width={28} height={28} className="shrink-0" />
              <div className="font-mono text-lg font-semibold text-primary">sdr--</div>
            </div>
            <CardTitle>Connect to sdr--</CardTitle>
            <CardDescription>
              This server requires its shared token (the value it was started with as
              <code className="mx-1 font-mono">--token</code>).
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Field data-invalid={refused || undefined}>
              <FieldLabel htmlFor="shared-token">Shared token</FieldLabel>
              <Input
                id="shared-token"
                type="password"
                autoComplete="current-password"
                aria-invalid={refused || undefined}
                value={entry}
                onChange={(e) => setEntry(e.target.value)}
              />
              {refused && <FieldError>That token was refused.</FieldError>}
            </Field>
          </CardContent>
          <CardFooter>
            <Button type="submit" disabled={entry.trim() === ""}>
              Connect
            </Button>
          </CardFooter>
        </form>
      </Card>
    </div>
  );
}

/** Forget a token the server has rejected, so the gate asks again instead of the client
 * retrying a credential that will never work. */
export function clearToken(): void {
  setToken(null);
}

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { renderSVG } from "uqr";
import { Button } from "../components/BaseControls";
import { BTN, BTN_PRIMARY } from "../components/controls";
import { pairRemote, REMOTE_KEY, remoteQuery, unpairRemote } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { RemoteStatus } from "../lib/types";
import { appHost, remoteAction, remoteLine, remotePollMs } from "./remote";

export function RemotePanel() {
  const queryClient = useQueryClient();
  const remote = useQuery(remoteQuery(remotePollMs));
  const settled = {
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: REMOTE_KEY }),
  };
  const pair = useMutation({
    mutationFn: pairRemote,
    onSuccess: (status: RemoteStatus) => queryClient.setQueryData(REMOTE_KEY, status),
    ...settled,
  });
  const unpair = useMutation({ mutationFn: unpairRemote, ...settled });
  if (remote.data === undefined) {
    return null;
  }
  return (
    <RemoteView
      status={remote.data}
      busy={pair.isPending || unpair.isPending}
      onPair={() => pair.mutate()}
      onUnpair={() => unpair.mutate()}
    />
  );
}

export function RemoteView({
  status,
  busy,
  onPair,
  onUnpair,
}: {
  status: RemoteStatus;
  busy: boolean;
  onPair: () => void;
  onUnpair: () => void;
}) {
  const host = appHost(status.app_origin);
  const action = remoteAction(status);
  const line = remoteLine(status);
  return (
    <div className="mx-auto flex w-72 max-w-full flex-col items-center gap-2 p-3">
      {status.state === "pairing" && <PairingCode status={status} host={host} />}
      {line !== "" && (
        <p
          className={`text-center text-xs ${status.state === "online" ? "text-ok" : "text-ink-dim"}`}
        >
          {line}
        </p>
      )}
      {status.state === "unpaired" && status.error && (
        <p role="alert" className="text-center text-xs text-danger">
          {status.error}
        </p>
      )}
      {status.state !== "unpaired" && status.state !== "pairing" && (
        <a
          href={status.app_origin}
          target="_blank"
          rel="noreferrer"
          className="text-xs text-ink hover:text-accent hover:underline"
        >
          Open {host}
        </a>
      )}
      {action === "pair" && (
        <Button type="button" className={BTN_PRIMARY} disabled={busy} onClick={onPair}>
          Connect to {host}
        </Button>
      )}
      {action === "pair-again" && (
        <Button type="button" className={BTN_PRIMARY} disabled={busy} onClick={onPair}>
          Pair again
        </Button>
      )}
      {action === "cancel" && (
        <Button type="button" className={BTN} disabled={busy} onClick={onUnpair}>
          Cancel
        </Button>
      )}
      {action === "disconnect" && (
        <Button type="button" className={BTN} disabled={busy} onClick={onUnpair}>
          Disconnect
        </Button>
      )}
    </div>
  );
}

function PairingCode({ status, host }: { status: RemoteStatus; host: string }) {
  const url = status.verification_uri_complete ?? status.verification_uri ?? status.app_origin;
  return (
    <>
      <div className="font-mono text-2xl font-semibold tracking-widest text-accent">
        {status.user_code}
      </div>
      <img
        className="size-40 rounded bg-white p-2"
        alt="Pairing QR code"
        src={`data:image/svg+xml;utf8,${encodeURIComponent(renderSVG(url, { border: 1 }))}`}
      />
      <a
        href={url}
        target="_blank"
        rel="noreferrer"
        className="text-xs text-ink hover:text-accent hover:underline"
      >
        Approve on {host}
      </a>
    </>
  );
}

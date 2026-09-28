import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button, Input } from "../components/BaseControls";
import { BTN, BTN_DANGER_SM, BTN_PRIMARY, BTN_SM, FIELD, LABEL } from "../components/controls";
import { List, ListRow, Panel, PanelHint } from "../components/ListPanel";
import { NumberField } from "../components/NumberField";
import { QrCode } from "../components/QrCode";
import { Switch } from "../components/Switch";
import {
  cancelPairingOffer,
  createPairingOffer,
  PHONES_KEY,
  phonesQuery,
  renamePhone,
  revokePhone,
  setPhoneAccess,
} from "../lib/api";
import {
  countdownLabel,
  discoveryLine,
  formatPairingCode,
  listenerLine,
  OFFER_POLL_MS,
  type OfferView,
  offerIsLive,
  offerView,
  PLATFORM_LABEL,
  pairedPhone,
  type StatusLine,
  seenLabel,
  type Tone,
} from "../lib/phones";
import { pushToast } from "../lib/toasts";
import type { PairingOffer, Phone, PhoneAccess, PhonesResponse } from "../lib/types";
import { useNow } from "../lib/useNow";

const TICK_MS = 1_000;
const MAX_PORT = 65_535;

const TONE: Record<Tone, string> = {
  ok: "text-ok",
  danger: "text-danger",
  dim: "text-ink-dim",
};

export interface PhoneActions {
  access: (access: PhoneAccess) => void;
  pair: () => void;
  cancel: () => void;
  edit: (id: string | null) => void;
  rename: (id: string, name: string) => void;
  confirm: (id: string | null) => void;
  revoke: (id: string) => void;
}

export interface PhonesViewProps {
  data: PhonesResponse | undefined;
  failed: boolean;
  now: number;
  editing: string | null;
  confirming: string | null;
  busy: boolean;
  on: PhoneActions;
}

function withOffer(
  data: PhonesResponse | undefined,
  offer: PairingOffer,
): PhonesResponse | undefined {
  return (
    data && {
      ...data,
      offer: {
        id: offer.id,
        state: { state: "live" },
        expires_at: offer.expires_at,
        failures: 0,
        code: offer.code,
        uri: offer.uri,
      },
    }
  );
}

function fail(error: Error): void {
  pushToast(error.message);
}

function usePairedToast(data: PhonesResponse | undefined): void {
  const watched = useRef<string | null>(null);
  const offer = data?.offer;
  const phones = data?.phones;
  useEffect(() => {
    if (offer == null || phones === undefined) {
      return;
    }
    if (offerIsLive(offer)) {
      watched.current = offer.id;
      return;
    }
    if (watched.current !== offer.id) {
      return;
    }
    watched.current = null;
    const phone = pairedPhone(offer, phones);
    if (phone !== null) {
      pushToast(`Paired ${phone.name}`, "info");
    }
  }, [offer, phones]);
}

export function PhonesPanel() {
  const queryClient = useQueryClient();
  const phones = useQuery({
    ...phonesQuery(),
    refetchInterval: (query) => (offerIsLive(query.state.data?.offer) ? OFFER_POLL_MS : false),
  });
  const now = useNow(TICK_MS);
  const [editing, setEditing] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  usePairedToast(phones.data);

  const refresh = (): void => {
    void queryClient.invalidateQueries({ queryKey: PHONES_KEY });
  };
  const access = useMutation({
    mutationFn: setPhoneAccess,
    onSuccess: (status) =>
      queryClient.setQueryData<PhonesResponse>(
        PHONES_KEY,
        (data) => data && { ...data, access: status },
      ),
    onError: fail,
    onSettled: refresh,
  });
  const pair = useMutation({
    mutationFn: () => createPairingOffer(),
    onSuccess: (offer) =>
      queryClient.setQueryData<PhonesResponse>(PHONES_KEY, (data) => withOffer(data, offer)),
    onError: fail,
    onSettled: refresh,
  });
  const cancel = useMutation({ mutationFn: cancelPairingOffer, onError: fail, onSettled: refresh });
  const rename = useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => renamePhone(id, name),
    onError: fail,
    onSettled: refresh,
  });
  const revoke = useMutation({
    mutationFn: revokePhone,
    onSuccess: () => setConfirming(null),
    onError: fail,
    onSettled: refresh,
  });

  return (
    <PhonesView
      data={phones.data}
      failed={phones.isError}
      now={now}
      editing={editing}
      confirming={confirming}
      busy={access.isPending || pair.isPending || cancel.isPending || revoke.isPending}
      on={{
        access: (next) => access.mutate(next),
        pair: () => pair.mutate(),
        cancel: () => cancel.mutate(),
        edit: setEditing,
        rename: (id, name) => {
          setEditing(null);
          rename.mutate({ id, name });
        },
        confirm: setConfirming,
        revoke: (id) => revoke.mutate(id),
      }}
    />
  );
}

function ListFailed() {
  return (
    <p role="alert" className="text-xs text-danger">
      Phone list failed
    </p>
  );
}

export function PhonesView({ data, failed, now, editing, confirming, busy, on }: PhonesViewProps) {
  if (data === undefined) {
    return <Panel>{failed ? <ListFailed /> : <PanelHint>Loading</PanelHint>}</Panel>;
  }
  const view = offerView(data.offer, now);
  const ready = data.access.endpoint != null;
  return (
    <Panel>
      {failed && <ListFailed />}
      <AccessSettings data={data} busy={busy} onAccess={on.access} />
      {view === null ? (
        <Button
          type="button"
          className={`${BTN_PRIMARY} self-start`}
          disabled={!ready || busy}
          title={ready ? undefined : "Allow phones first"}
          onClick={on.pair}
        >
          Pair phone
        </Button>
      ) : (
        <OfferCard
          view={view}
          keyCheck={data.access.endpoint?.key_check ?? null}
          busy={busy}
          onCancel={on.cancel}
          onNew={on.pair}
        />
      )}
      {data.phones.length === 0 ? (
        <PanelHint>No phones</PanelHint>
      ) : (
        <List>
          <PhoneRows
            phones={data.phones}
            now={now}
            editing={editing}
            confirming={confirming}
            busy={busy}
            on={on}
          />
        </List>
      )}
    </Panel>
  );
}

function Status({ line }: { line: StatusLine }) {
  return (
    <span className={TONE[line.tone]} title={line.title}>
      {line.text}
    </span>
  );
}

export function AccessSettings({
  data,
  busy,
  onAccess,
}: {
  data: PhonesResponse;
  busy: boolean;
  onAccess: (access: PhoneAccess) => void;
}) {
  const { enabled, port } = data.access.access;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <Switch
          label="Allow phones"
          checked={enabled}
          onChange={(next) => onAccess({ enabled: next, port })}
        />
        <span className="text-xs text-ink" title="Listen for phones over HTTPS">
          Allow phones
        </span>
        <span className={`${LABEL} ml-auto`} title="Phone port">
          Port
        </span>
        <NumberField
          label="Port"
          value={port}
          min={1}
          max={MAX_PORT}
          step={1}
          disabled={busy}
          className="w-20"
          onCommit={(next) => onAccess({ enabled, port: next })}
        />
      </div>
      <div className="flex items-center justify-between gap-2 font-mono text-[10.5px]">
        <Status line={listenerLine(data.access)} />
        <Status line={discoveryLine(data.access)} />
      </div>
    </div>
  );
}

export function OfferCard({
  view,
  keyCheck,
  busy,
  onCancel,
  onNew,
}: {
  view: OfferView;
  keyCheck: string | null;
  busy: boolean;
  onCancel: () => void;
  onNew: () => void;
}) {
  if (view.kind !== "live") {
    return (
      <div className="flex items-center gap-2 rounded-[3px] border border-line p-2">
        <span role="alert" className="flex-1 text-xs text-danger">
          {view.kind === "burned" ? "Too many tries" : "Expired"}
        </span>
        <Button type="button" className={BTN} disabled={busy} onClick={onNew}>
          New code
        </Button>
      </div>
    );
  }
  return (
    <section
      aria-label="Pairing"
      className="flex flex-col items-center gap-1.5 rounded-[3px] border border-line p-2"
    >
      {view.uri !== null && <QrCode value={view.uri} label="Pairing QR code" />}
      <span className="flex items-baseline gap-2">
        <span className={LABEL}>Code</span>
        <span
          className="font-mono text-lg tabular-nums text-ink"
          title="Type this on the phone if the camera fails"
        >
          {formatPairingCode(view.code)}
        </span>
      </span>
      {keyCheck !== null && (
        <span className="flex items-baseline gap-2">
          <span className={LABEL}>Key</span>
          <span className="font-mono text-xs text-ink" title="Check this on the phone">
            {keyCheck}
          </span>
        </span>
      )}
      <span className="flex w-full items-center justify-between gap-2">
        <span className="font-mono text-xs tabular-nums text-ink-dim" title="Code expires">
          {countdownLabel(view.remainingMs)}
        </span>
        <Button type="button" className={BTN_SM} disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
      </span>
    </section>
  );
}

export function PhoneRows({
  phones,
  now,
  editing,
  confirming,
  busy,
  on,
}: {
  phones: readonly Phone[];
  now: number;
  editing: string | null;
  confirming: string | null;
  busy: boolean;
  on: PhoneActions;
}) {
  return phones.map((phone) => (
    <ListRow
      key={phone.id}
      lead={
        <span
          aria-hidden
          className={`size-2 shrink-0 rounded-full ${phone.online ? "bg-ok" : "bg-ink-faint"}`}
        />
      }
      primary={
        editing === phone.id ? (
          <NameInput phone={phone} on={on} />
        ) : (
          <Button
            type="button"
            className="max-w-full truncate text-left hover:text-accent"
            title="Rename"
            onClick={() => on.edit(phone.id)}
          >
            {phone.name}
          </Button>
        )
      }
      secondary={
        <>
          {PLATFORM_LABEL[phone.platform]} ·{" "}
          <span title={phone.last_seen ?? undefined}>{seenLabel(phone, now)}</span>
        </>
      }
      actions={
        confirming === phone.id ? (
          <>
            <Button
              type="button"
              className={BTN_DANGER_SM}
              disabled={busy}
              onClick={() => on.revoke(phone.id)}
            >
              Revoke?
            </Button>
            <Button type="button" className={BTN_SM} onClick={() => on.confirm(null)}>
              Cancel
            </Button>
          </>
        ) : (
          <Button
            type="button"
            className={BTN_SM}
            aria-label={`Revoke ${phone.name}`}
            title="Revoke"
            onClick={() => on.confirm(phone.id)}
          >
            Revoke
          </Button>
        )
      }
    />
  ));
}

function NameInput({ phone, on }: { phone: Phone; on: PhoneActions }) {
  return (
    <Input
      className={`${FIELD} w-full`}
      aria-label="Phone name"
      defaultValue={phone.name}
      autoFocus
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.currentTarget.blur();
        } else if (event.key === "Escape") {
          event.currentTarget.value = phone.name;
          on.edit(null);
        }
      }}
      onBlur={(event) => {
        const name = event.currentTarget.value.trim();
        if (name === "" || name === phone.name) {
          on.edit(null);
        } else {
          on.rename(phone.id, name);
        }
      }}
    />
  );
}

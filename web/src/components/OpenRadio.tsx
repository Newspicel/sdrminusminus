import { Collapsible } from "@base-ui/react/collapsible";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BookmarkPlus, Trash2 } from "lucide-react";
import { useState } from "react";
import {
  deleteSavedRadio,
  devicesQuery,
  doctorQuery,
  SAVED_RADIOS_KEY,
  savedRadiosQuery,
  saveRadio,
} from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceInfo, DeviceRef } from "../lib/types";
import { Button, Form, Input } from "./BaseControls";
import { BTN, BTN_QUIET, FIELD, ICON_BTN, LABEL } from "./controls";
import {
  deviceHint,
  deviceId,
  groupDevices,
  NETWORK_BACKENDS,
  networkDeviceId,
  networkRadioLabel,
  type SourceTab,
  sourceTabs,
  unclaimedDevices,
  visibleDevices,
} from "./devices";
import { Icon } from "./Icon";
import { List, ListRow, RowAction } from "./ListPanel";
import { Segmented } from "./Segmented";
import { Select } from "./Select";

type Choose = (device: DeviceInfo) => void;

function useSavedRadios() {
  const queryClient = useQueryClient();
  const saved = useQuery(savedRadiosQuery());
  const settled = {
    onError: (error: Error) => pushToast(error.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: SAVED_RADIOS_KEY }),
  };
  const save = useMutation({
    mutationFn: (id: string) => saveRadio({ device_id: id, label: networkRadioLabel(id) }),
    ...settled,
  });
  const forget = useMutation({ mutationFn: deleteSavedRadio, ...settled });
  return { radios: saved.data ?? [], save, forget };
}

function SavedRadios({ onAdd, busy }: { onAdd: (id: string) => void; busy: boolean }) {
  const { radios, forget } = useSavedRadios();
  if (radios.length === 0) {
    return null;
  }
  return (
    <List title="Saved">
      {radios.map((radio) => (
        <ListRow
          key={radio.id}
          primary={radio.label}
          disabled={busy}
          onSelect={() => onAdd(radio.device_id)}
          actions={
            <RowAction
              label={`Forget ${radio.label}`}
              glyph={Trash2}
              danger
              disabled={forget.isPending}
              onClick={() => forget.mutate(radio.id)}
            />
          }
        />
      ))}
    </List>
  );
}

function AddNetworkRadio({ onAdd, busy }: { onAdd: (id: string) => void; busy: boolean }) {
  const { save } = useSavedRadios();
  const [driver, setDriver] = useState<string>(NETWORK_BACKENDS[0].driver);
  const [address, setAddress] = useState("");
  const backend = NETWORK_BACKENDS.find((b) => b.driver === driver) ?? NETWORK_BACKENDS[0];
  const id = networkDeviceId(driver, address);
  const port = backend.placeholder.split(":").pop();

  return (
    <Form
      className="flex flex-col gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (id !== null) {
          onAdd(id);
        }
      }}
    >
      <div className="flex items-center gap-2">
        <span className={LABEL}>Via</span>
        <Select
          label="Network protocol"
          value={driver}
          options={NETWORK_BACKENDS.map((b) => ({ value: b.driver, label: b.label }))}
          onChange={setDriver}
        />
      </div>
      <div className="flex items-center gap-2">
        <Input
          className={`${FIELD} w-full`}
          type="text"
          aria-label="Radio address"
          title={`Host or address of the ${backend.label} server; without a port it uses ${port}`}
          placeholder={backend.placeholder}
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
        <Button
          type="button"
          className={ICON_BTN}
          aria-label="Save radio"
          title="Save for later"
          disabled={save.isPending || id === null}
          onClick={() => id !== null && save.mutate(id)}
        >
          <Icon glyph={BookmarkPlus} size={14} />
        </Button>
        <Button type="submit" className={BTN} disabled={busy || id === null}>
          Add
        </Button>
      </div>
    </Form>
  );
}

function RadioList({
  devices,
  busy,
  onChoose,
}: {
  devices: readonly DeviceInfo[];
  busy: boolean;
  onChoose: Choose;
}) {
  return (
    <div className="flex flex-col gap-1">
      {devices.map((device) => (
        <Button
          key={deviceId(device)}
          type="button"
          className={`${BTN} justify-center`}
          title={deviceHint(device)}
          disabled={busy}
          onClick={() => onChoose(device)}
        >
          <span className="truncate">{device.label}</span>
        </Button>
      ))}
    </div>
  );
}

function Radios({
  radios,
  pending,
  elsewhere,
  busy,
  onChoose,
}: {
  radios: readonly DeviceInfo[];
  pending: boolean;
  elsewhere: number;
  busy: boolean;
  onChoose: Choose;
}) {
  return (
    <>
      <RadioList devices={radios} busy={busy} onChoose={onChoose} />
      {pending && <p className="text-sm text-ink-dim">Looking for radios…</p>}
      {!pending && radios.length === 0 && (
        <p
          className="text-sm text-ink-dim"
          title={
            elsewhere > 0
              ? "Plug another radio in, or move that node's wires here"
              : "Plug a radio in, or pick a recording or a network radio above"
          }
        >
          {elsewhere > 0
            ? "Every radio found is already open on another node."
            : "No radios found."}
        </p>
      )}
      <HardwareCheck />
    </>
  );
}

export function DeviceChoices({
  onChoose,
  onAddNetwork,
  busy = false,
  error = null,
  claimed = [],
}: {
  onChoose: Choose;
  onAddNetwork: (deviceId: string) => void;
  busy?: boolean;
  error?: string | null;
  claimed?: readonly DeviceRef[];
}) {
  const devices = useQuery(devicesQuery());
  const [tab, setTab] = useState<SourceTab>("radios");
  const visible = visibleDevices(devices.data?.devices ?? []);
  const found = unclaimedDevices(visible, claimed);
  const groups = groupDevices(found);
  const tabs = sourceTabs(groups);
  const shown = tabs.some((option) => option.value === tab) ? tab : "radios";

  return (
    <div className="flex w-full flex-col gap-2">
      <Segmented label="Radio source" value={shown} options={tabs} onChange={setTab} fill />

      {error !== null && (
        <p role="alert" className="font-mono text-xs text-danger">
          {error}
        </p>
      )}

      {shown === "radios" && (
        <Radios
          radios={groups.radios}
          pending={devices.isPending}
          elsewhere={visible.length - found.length}
          busy={busy}
          onChoose={onChoose}
        />
      )}
      {shown === "network" && (
        <>
          <SavedRadios onAdd={onAddNetwork} busy={busy} />
          <AddNetworkRadio onAdd={onAddNetwork} busy={busy} />
        </>
      )}
      {shown === "virtual" && (
        <RadioList devices={groups.virtual} busy={busy} onChoose={onChoose} />
      )}
    </div>
  );
}

function HardwareCheck() {
  return (
    <Collapsible.Root>
      <Collapsible.Trigger
        className={BTN_QUIET}
        title="Run the checks behind sdrmm --doctor: drivers, permissions, and what the USB bus reports"
      >
        Check hardware
      </Collapsible.Trigger>
      <Collapsible.Panel className="pt-2">
        <Doctor />
      </Collapsible.Panel>
    </Collapsible.Root>
  );
}

function Doctor() {
  const doctor = useQuery(doctorQuery(true));
  if (doctor.isPending) {
    return <p className="text-sm text-ink-dim">Checking…</p>;
  }
  if (doctor.error) {
    return (
      <p role="alert" className="font-mono text-xs text-danger">
        Diagnostics failed: {doctor.error.message}
      </p>
    );
  }
  return (
    <dl className="flex w-full flex-col gap-2 text-left">
      {(doctor.data?.checks ?? []).map((check) => (
        <div key={check.id}>
          <dt className="flex items-center gap-2 font-mono text-xs">
            <span
              className={
                check.status === "fail"
                  ? "text-danger"
                  : check.status === "warn"
                    ? "text-ink"
                    : "text-ok"
              }
            >
              [{check.status}]
            </span>
            <span className="text-ink">{check.name}</span>
          </dt>
          <dd className="legend pl-4 whitespace-pre-wrap normal-case">
            {check.detail}
            {check.hint != null && `\n→ ${check.hint}`}
          </dd>
        </div>
      ))}
    </dl>
  );
}

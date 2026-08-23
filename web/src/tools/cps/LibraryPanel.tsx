import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import { BTN_SM, FIELD, LABEL } from "../../components/controls";
import { Select } from "../../components/Select";
import {
  CPS_LIBRARY_KEY,
  cpsLibraryQuery,
  createCpsDevice,
  createCpsUser,
  deleteCpsCodeplug,
  deleteCpsDevice,
  deleteCpsUser,
  radioModelsQuery,
} from "../../lib/api";
import type { CpsCodeplugInfo } from "../../lib/types";
import { modelLabel } from "./cps";

const ROW = "flex items-baseline justify-between gap-2 border-t border-line py-1 text-xs";

export function LibraryPanel({
  selected,
  onSelect,
}: {
  selected: number | null;
  onSelect: (id: number | null) => void;
}) {
  const library = useQuery(cpsLibraryQuery());
  const models = useQuery(radioModelsQuery());
  const queryClient = useQueryClient();
  const refresh = () => queryClient.invalidateQueries({ queryKey: CPS_LIBRARY_KEY });

  const [operator, setOperator] = useState({ name: "", callsign: "", dmrId: "" });
  const [radioName, setRadioName] = useState("");
  const [radioModel, setRadioModel] = useState("");

  const addOperator = useMutation({
    mutationFn: async () =>
      createCpsUser({
        name: operator.name.trim(),
        callsign: operator.callsign.trim() === "" ? undefined : operator.callsign.trim(),
        dmr_id: operator.dmrId.trim() === "" ? undefined : Number(operator.dmrId),
      }),
    onSuccess: async () => {
      setOperator({ name: "", callsign: "", dmrId: "" });
      await refresh();
    },
  });

  const modelOptions = (models.data?.models ?? []).map((model) => ({
    value: model.id,
    label: modelLabel(model),
  }));
  const modelId = radioModel === "" ? (modelOptions[0]?.value ?? "") : radioModel;

  const addRadio = useMutation({
    mutationFn: async () => createCpsDevice({ name: radioName.trim(), model_id: modelId }),
    onSuccess: async () => {
      setRadioName("");
      await refresh();
    },
  });

  const drop = useMutation({
    mutationFn: async (target: { kind: "user" | "device" | "codeplug"; id: number }) => {
      if (target.kind === "user") {
        await deleteCpsUser(target.id);
      } else if (target.kind === "device") {
        await deleteCpsDevice(target.id);
      } else {
        await deleteCpsCodeplug(target.id);
      }
    },
    onSuccess: refresh,
  });

  const codeplugs = library.data?.codeplugs ?? [];

  return (
    <div className="flex min-h-0 flex-col gap-4 overflow-auto pr-1">
      <section className="flex flex-col gap-1">
        <h3 className={LABEL}>Operators</h3>
        {(library.data?.users ?? []).map((user) => (
          <div key={user.id} className={ROW}>
            <span className="min-w-0 truncate text-ink">
              {user.callsign ?? user.name}
              {user.dmr_id === undefined ? "" : ` · ${user.dmr_id}`}
            </span>
            <Button
              type="button"
              className={BTN_SM}
              onClick={() => drop.mutate({ kind: "user", id: user.id })}
            >
              Remove
            </Button>
          </div>
        ))}
        <div className="flex flex-wrap gap-1 pt-1">
          <Input
            className={`${FIELD} w-28`}
            aria-label="Operator name"
            placeholder="Name"
            value={operator.name}
            onChange={(event) => setOperator({ ...operator, name: event.target.value })}
          />
          <Input
            className={`${FIELD} w-24`}
            aria-label="Callsign"
            placeholder="Callsign"
            value={operator.callsign}
            onChange={(event) => setOperator({ ...operator, callsign: event.target.value })}
          />
          <Input
            className={`${FIELD} w-24`}
            aria-label="DMR ID"
            placeholder="DMR ID"
            inputMode="numeric"
            value={operator.dmrId}
            onChange={(event) => setOperator({ ...operator, dmrId: event.target.value })}
          />
          <Button
            type="button"
            className={BTN_SM}
            disabled={operator.name.trim() === "" || addOperator.isPending}
            onClick={() => addOperator.mutate()}
          >
            Add
          </Button>
        </div>
        {addOperator.error !== null && (
          <p className="text-xs text-danger">{addOperator.error.message}</p>
        )}
      </section>

      <section className="flex flex-col gap-1">
        <h3 className={LABEL}>Radios</h3>
        {(library.data?.devices ?? []).map((device) => (
          <div key={device.id} className={ROW}>
            <span className="min-w-0 truncate text-ink">
              {device.name} <span className="text-ink-faint">{device.model_id}</span>
            </span>
            <Button
              type="button"
              className={BTN_SM}
              onClick={() => drop.mutate({ kind: "device", id: device.id })}
            >
              Remove
            </Button>
          </div>
        ))}
        <div className="flex flex-wrap items-center gap-1 pt-1">
          <Input
            className={`${FIELD} w-28`}
            aria-label="Radio name"
            placeholder="Name"
            value={radioName}
            onChange={(event) => setRadioName(event.target.value)}
          />
          <Select
            label="Radio model"
            className="w-40"
            value={modelId}
            options={modelOptions}
            onChange={setRadioModel}
          />
          <Button
            type="button"
            className={BTN_SM}
            disabled={radioName.trim() === "" || modelId === "" || addRadio.isPending}
            onClick={() => addRadio.mutate()}
          >
            Add
          </Button>
        </div>
        {addRadio.error !== null && <p className="text-xs text-danger">{addRadio.error.message}</p>}
      </section>

      <section className="flex min-h-0 flex-col gap-1">
        <h3 className={LABEL}>Codeplugs</h3>
        {codeplugs.length === 0 && (
          <p className="py-1 text-xs text-ink-dim">
            Nothing stored yet. Read a radio, or build one and copy it across.
          </p>
        )}
        {codeplugs.map((info) => (
          <CodeplugRow
            key={info.id}
            info={info}
            selected={info.id === selected}
            onSelect={() => onSelect(info.id === selected ? null : info.id)}
            onDelete={() => drop.mutate({ kind: "codeplug", id: info.id })}
          />
        ))}
      </section>
    </div>
  );
}

function CodeplugRow({
  info,
  selected,
  onSelect,
  onDelete,
}: {
  info: CpsCodeplugInfo;
  selected: boolean;
  onSelect: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className={`${ROW} rounded-[3px] px-1 transition-colors duration-100 ${
        selected ? "bg-panel-2" : ""
      }`}
    >
      <Button type="button" className="min-w-0 flex-1 text-left" onClick={onSelect}>
        <span className="block truncate text-ink">{info.name}</span>
        <span className="block truncate font-mono text-[10px] text-ink-faint">
          {info.model_id} · {info.counts.channels} ch · {info.counts.contacts} contacts ·{" "}
          {info.counts.zones} zones
        </span>
      </Button>
      <Button type="button" className={BTN_SM} onClick={onDelete}>
        Remove
      </Button>
    </div>
  );
}

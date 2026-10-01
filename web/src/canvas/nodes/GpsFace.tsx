import { useQuery } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { BTN_QUIET } from "../../components/controls";
import { ChipField, Chips, ChoiceChip, NumberChip, SettingChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { TextChip } from "../../components/face/TextChip";
import { TextAutocomplete } from "../../components/TextAutocomplete";
import { nmeaDevicesQuery, phonesQuery } from "../../lib/api";
import {
  fixAgeLabel,
  headingLabel,
  PHONE_NOT_PAIRED,
  PHONE_OFFLINE,
  type PhoneStanding,
  phoneStanding,
  STANDING_LABEL,
  tiltLabel,
} from "../../lib/phones";
import { gridLocator, usePositionStore } from "../../lib/position";
import type { PatchNode, PositionFix, PositionSource } from "../../lib/types";
import { useNow } from "../../lib/useNow";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { GpsChoices } from "./GpsChoices";
import { nmeaSuggestion, validGpsdAddress } from "./gpsSource";
import { FaceBody, FaceFooter, NodeShell } from "./NodeShell";

type Source<T extends PositionSource["type"]> = Extract<PositionSource, { type: T }>;

type Change = (source: PositionSource) => void;

export function GpsFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = usePositionStore((store) => store.sources[node.id]);
  if (node.kind !== "gps") {
    return null;
  }
  const source = node.data.source ?? null;
  const setSource = (next: PositionSource | null): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "gps"
          ? { ...current, data: next === null ? {} : { source: next } }
          : current,
      ),
    }));
  };
  const fix = state?.fix ?? null;
  const error = state?.error ?? null;
  if (source === null) {
    return (
      <NodeShell node={node} title="GPS position" category="source">
        <FaceBody>
          <div className="flex flex-col gap-2 p-2">
            <GpsChoices onChoose={setSource} />
          </div>
        </FaceBody>
      </NodeShell>
    );
  }
  return (
    <NodeShell
      node={node}
      title="GPS position"
      category="source"
      subtitle={
        source.type === "phone" ? (
          <PhoneStandingSaid phone={source.phone} error={error} />
        ) : undefined
      }
    >
      <FaceBody>
        {fix !== null && <FixReadout fix={fix} phone={source.type === "phone"} />}
        <Chips className="p-2">
          <SourceChips source={source} onChange={setSource} />
        </Chips>
        {source.type === "phone" && <PhoneListFault />}
        {source.type === "nmea" && <SerialListFault />}
        {fix === null && error !== null && !shownAsStanding(source, error) && (
          <FaceFault message={error} />
        )}
      </FaceBody>
      <FaceFooter>
        <Button
          type="button"
          className={BTN_QUIET}
          title="Free this node so you can pick a different source"
          onClick={() => setSource(null)}
        >
          Forget source
        </Button>
      </FaceFooter>
    </NodeShell>
  );
}

const STANDING_TONE: Record<PhoneStanding, string> = {
  online: "text-ok",
  offline: "text-ink-dim",
  not_paired: "text-danger",
};

const FIX_TICK_MS = 1_000;

const UPDATE_RATES = [
  { value: 1_000, label: "1 Hz" },
  { value: 500, label: "2 Hz" },
  { value: 200, label: "5 Hz" },
  { value: 100, label: "10 Hz" },
  { value: 50, label: "20 Hz" },
];

const BAUDS = [4_800, 9_600, 38_400, 57_600, 115_200].map((baud) => ({ value: String(baud) }));

function shownAsStanding(source: PositionSource, error: string): boolean {
  return source.type === "phone" && (error === PHONE_OFFLINE || error === PHONE_NOT_PAIRED);
}

function PhoneStandingSaid({ phone, error }: { phone: string; error: string | null }) {
  const phones = useQuery(phonesQuery());
  const standing = phoneStanding(phones.data?.phones, phone, error);
  return standing === null ? null : (
    <span className={STANDING_TONE[standing]}>{STANDING_LABEL[standing]}</span>
  );
}

function PhoneListFault() {
  const phones = useQuery(phonesQuery());
  return phones.isError ? (
    <FaceFault message="Phone list failed" detail={phones.error.message} />
  ) : null;
}

function SerialListFault() {
  const devices = useQuery(nmeaDevicesQuery());
  return devices.isError ? (
    <FaceFault message="Serial device discovery failed" detail={devices.error.message} />
  ) : null;
}

function FixReadout({ fix, phone }: { fix: PositionFix; phone: boolean }) {
  const tilt = tiltLabel(fix);
  return (
    <Readouts>
      <Readout label="Position">
        {fix.latitude.toFixed(6)}, {fix.longitude.toFixed(6)}
      </Readout>
      <Readout label="Grid">{gridLocator(fix.latitude, fix.longitude)}</Readout>
      {fix.accuracy_m != null && <Readout label="Accuracy">±{fix.accuracy_m.toFixed(0)} m</Readout>}
      {fix.speed_mps != null && (
        <Readout label="Speed">{(fix.speed_mps * 3.6).toFixed(1)} km/h</Readout>
      )}
      {(phone || fix.heading_deg != null) && (
        <Readout label="Heading" title="True north">
          {headingLabel(fix)}
        </Readout>
      )}
      {tilt !== null && (
        <Readout label="Tilt" title="Pitch and roll">
          {tilt}
        </Readout>
      )}
      {phone && <FixAge time={fix.time} />}
    </Readouts>
  );
}

function FixAge({ time }: { time: string }) {
  const now = useNow(FIX_TICK_MS);
  return <Readout label="Fix">{fixAgeLabel(time, now)}</Readout>;
}

function SourceChips({ source, onChange }: { source: PositionSource; onChange: Change }) {
  switch (source.type) {
    case "gpsd":
      return (
        <TextChip
          label="gpsd"
          name="GPSD address"
          title="Host and port of the gpsd daemon"
          value={source.address}
          onCommit={(address) => {
            if (validGpsdAddress(address)) {
              onChange({ type: "gpsd", address });
            }
          }}
        />
      );
    case "fixed":
      return <FixedChips source={source} onChange={onChange} />;
    case "nmea":
      return <NmeaChips source={source} onChange={onChange} />;
    case "phone":
      return <PhoneChip source={source} onChange={onChange} />;
  }
}

function PhoneChip({ source, onChange }: { source: Source<"phone">; onChange: Change }) {
  const phones = useQuery(phonesQuery());
  const options = (phones.data?.phones ?? []).map((phone) => ({
    value: phone.id,
    label: phone.name,
  }));
  if (!options.some((option) => option.value === source.phone)) {
    options.push({ value: source.phone, label: source.phone });
  }
  return (
    <ChoiceChip
      label="Phone"
      title="Paired phone giving the position"
      value={source.phone}
      options={options}
      onChange={(phone) => onChange({ type: "phone", phone })}
    />
  );
}

function FixedChips({ source, onChange }: { source: Source<"fixed">; onChange: Change }) {
  return (
    <>
      <NumberChip
        label="Lat"
        title="Latitude"
        value={source.lat}
        unit="°"
        min={-90}
        max={90}
        step={0.00001}
        onCommit={(lat) => onChange({ ...source, lat })}
      />
      <NumberChip
        label="Lon"
        title="Longitude"
        value={source.lon}
        unit="°"
        min={-180}
        max={180}
        step={0.00001}
        onCommit={(lon) => onChange({ ...source, lon })}
      />
    </>
  );
}

function NmeaChips({ source, onChange }: { source: Source<"nmea">; onChange: Change }) {
  const devices = useQuery(nmeaDevicesQuery());
  const updateInterval = source.update_interval_ms ?? 1_000;
  const none = devices.isSuccess && devices.data.devices.length === 0;
  return (
    <>
      <SettingChip label="Port" value={source.device} title="Serial port of the receiver">
        {() => (
          <ChipField label={none ? "Serial device, none detected" : "Serial device"}>
            <TextAutocomplete
              value={source.device}
              label="Serial device"
              className="min-w-0 flex-1"
              placeholder="Pick one or type a path"
              suggestions={(devices.data?.devices ?? []).map(nmeaSuggestion)}
              onCommit={(device) => {
                if (device !== source.device) {
                  onChange({ ...source, device, update_interval_ms: updateInterval });
                }
                return true;
              }}
            />
          </ChipField>
        )}
      </SettingChip>
      <SettingChip label="Baud" value={String(source.baud)} title="Serial baud rate" width="w-52">
        {() => (
          <ChipField label="Baud">
            <TextAutocomplete
              value={String(source.baud)}
              label="Baud"
              className="min-w-0 flex-1"
              inputMode="numeric"
              suggestions={BAUDS}
              onCommit={(value) => {
                const baud = Number(value);
                const valid = Number.isInteger(baud) && baud >= 1_200 && baud <= 4_000_000;
                if (valid && baud !== source.baud) {
                  onChange({ ...source, baud, update_interval_ms: updateInterval });
                }
                return valid;
              }}
            />
          </ChipField>
        )}
      </SettingChip>
      <ChoiceChip
        label="Rate"
        title="Update rate"
        value={updateInterval}
        options={UPDATE_RATES}
        onChange={(next) => onChange({ ...source, update_interval_ms: next })}
      />
    </>
  );
}

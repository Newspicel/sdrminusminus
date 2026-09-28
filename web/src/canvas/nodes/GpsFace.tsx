import { useQuery } from "@tanstack/react-query";
import { Button, Input } from "../../components/BaseControls";
import { BTN_QUIET, FIELD } from "../../components/controls";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { Select } from "../../components/Select";
import { SettingNote, SettingRow, Settings } from "../../components/Settings";
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
    <NodeShell node={node} title="GPS position" category="source">
      <FaceBody>
        <Settings className="p-2">
          <SourceSettings source={source} error={error} onChange={setSource} />
        </Settings>
        {fix === null ? (
          error !== null &&
          !shownAsStanding(source, error) && (
            <p role="alert" className="border-t border-line p-2 text-xs text-danger">
              {error}
            </p>
          )
        ) : (
          <FixReadout fix={fix} phone={source.type === "phone"} />
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

const STANDING_CHIP =
  "inline-flex h-5 items-center rounded-[3px] border border-line bg-well px-1.5 font-mono text-[10px]";

const FIX_TICK_MS = 1_000;

function shownAsStanding(source: PositionSource, error: string): boolean {
  return source.type === "phone" && (error === PHONE_OFFLINE || error === PHONE_NOT_PAIRED);
}

function FixReadout({ fix, phone }: { fix: PositionFix; phone: boolean }) {
  const tilt = tiltLabel(fix);
  return (
    <Readout>
      <ReadoutRow label="Position">
        {fix.latitude.toFixed(6)}, {fix.longitude.toFixed(6)}
      </ReadoutRow>
      <ReadoutRow label="Grid">{gridLocator(fix.latitude, fix.longitude)}</ReadoutRow>
      {fix.accuracy_m != null && (
        <ReadoutRow label="Accuracy">±{fix.accuracy_m.toFixed(0)} m</ReadoutRow>
      )}
      {fix.speed_mps != null && (
        <ReadoutRow label="Speed">{(fix.speed_mps * 3.6).toFixed(1)} km/h</ReadoutRow>
      )}
      {(phone || fix.heading_deg != null) && (
        <ReadoutRow label="Heading" title="True north">
          {headingLabel(fix)}
        </ReadoutRow>
      )}
      {tilt !== null && (
        <ReadoutRow label="Tilt" title="Pitch and roll">
          {tilt}
        </ReadoutRow>
      )}
      {phone && <FixAge time={fix.time} />}
    </Readout>
  );
}

function FixAge({ time }: { time: string }) {
  const now = useNow(FIX_TICK_MS);
  return <ReadoutRow label="Fix">{fixAgeLabel(time, now)}</ReadoutRow>;
}

function PhoneSettings({
  source,
  error,
  onChange,
}: {
  source: Extract<PositionSource, { type: "phone" }>;
  error: string | null;
  onChange: (source: PositionSource) => void;
}) {
  const phones = useQuery(phonesQuery());
  const listed = phones.data?.phones;
  const standing = phoneStanding(listed, source.phone, error);
  const options = (listed ?? []).map((phone) => ({ value: phone.id, label: phone.name }));
  if (!options.some((option) => option.value === source.phone)) {
    options.push({ value: source.phone, label: source.phone });
  }
  return (
    <SettingRow label="Phone">
      <Select
        label="Phone"
        value={source.phone}
        options={options}
        className="w-full max-w-40"
        onChange={(phone) => onChange({ type: "phone", phone })}
      />
      {standing !== null && (
        <span className={`${STANDING_CHIP} ${STANDING_TONE[standing]}`}>
          {STANDING_LABEL[standing]}
        </span>
      )}
      {phones.isError && <span className="text-xs text-danger">Phone list failed</span>}
    </SettingRow>
  );
}

function SourceSettings({
  source,
  error,
  onChange,
}: {
  source: PositionSource;
  error: string | null;
  onChange: (source: PositionSource) => void;
}) {
  switch (source.type) {
    case "gpsd":
      return (
        <SettingRow label="GPSD address">
          <Input
            key={source.address}
            aria-label="GPSD address"
            className={`${FIELD} w-full max-w-52`}
            defaultValue={source.address}
            onBlur={(event) => {
              const address = event.currentTarget.value.trim();
              if (!validGpsdAddress(address)) {
                event.currentTarget.value = source.address;
              } else if (address !== source.address) {
                onChange({ type: "gpsd", address });
              } else {
                event.currentTarget.value = source.address;
              }
            }}
          />
        </SettingRow>
      );
    case "fixed":
      return <FixedSettings source={source} onChange={onChange} />;
    case "nmea":
      return <NmeaSettings source={source} onChange={onChange} />;
    case "phone":
      return <PhoneSettings source={source} error={error} onChange={onChange} />;
  }
}

function FixedSettings({
  source,
  onChange,
}: {
  source: Extract<PositionSource, { type: "fixed" }>;
  onChange: (source: PositionSource) => void;
}) {
  return (
    <>
      <SettingRow label="Latitude">
        <NumberField
          label="Latitude"
          unit="°"
          value={source.lat}
          min={-90}
          max={90}
          step={0.00001}
          onCommit={(lat) => onChange({ ...source, lat })}
        />
      </SettingRow>
      <SettingRow label="Longitude">
        <NumberField
          label="Longitude"
          unit="°"
          value={source.lon}
          min={-180}
          max={180}
          step={0.00001}
          onCommit={(lon) => onChange({ ...source, lon })}
        />
      </SettingRow>
      <SettingRow label="Grid">
        <span className="font-mono text-sm">{gridLocator(source.lat, source.lon)}</span>
      </SettingRow>
    </>
  );
}

function NmeaSettings({
  source,
  onChange,
}: {
  source: Extract<PositionSource, { type: "nmea" }>;
  onChange: (source: PositionSource) => void;
}) {
  const devices = useQuery(nmeaDevicesQuery());
  const updateInterval = source.update_interval_ms ?? 1_000;
  return (
    <>
      <SettingRow label="Serial device">
        <TextAutocomplete
          value={source.device}
          label="Serial device"
          className="w-full max-w-52"
          placeholder="Choose a detected device or enter a path"
          suggestions={(devices.data?.devices ?? []).map(nmeaSuggestion)}
          onCommit={(device) => {
            if (device !== source.device) {
              onChange({ ...source, device, update_interval_ms: updateInterval });
            }
            return true;
          }}
        />
      </SettingRow>
      {devices.isError && (
        <p className="col-span-2 text-xs text-danger">Serial device discovery failed</p>
      )}
      {devices.isSuccess && devices.data.devices.length === 0 && (
        <SettingNote>No serial receiver detected: plug one in, or type its path.</SettingNote>
      )}
      <SettingRow label="Baud">
        <TextAutocomplete
          value={String(source.baud)}
          label="Baud"
          className="w-full max-w-52"
          inputMode="numeric"
          suggestions={[4_800, 9_600, 38_400, 57_600, 115_200].map((baud) => ({
            value: String(baud),
          }))}
          onCommit={(value) => {
            const baud = Number(value);
            const valid = Number.isInteger(baud) && baud >= 1_200 && baud <= 4_000_000;
            if (valid && baud !== source.baud) {
              onChange({ ...source, baud, update_interval_ms: updateInterval });
            }
            return valid;
          }}
        />
      </SettingRow>
      <SettingRow label="Update rate">
        <Select
          label="Update rate"
          value={updateInterval}
          options={[
            { value: 1_000, label: "1 Hz" },
            { value: 500, label: "2 Hz" },
            { value: 200, label: "5 Hz" },
            { value: 100, label: "10 Hz" },
            { value: 50, label: "20 Hz" },
          ]}
          onChange={(next) => onChange({ ...source, update_interval_ms: next })}
        />
      </SettingRow>
    </>
  );
}

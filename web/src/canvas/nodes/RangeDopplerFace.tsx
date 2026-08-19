import { useEffect, useRef } from "react";
import { Checkbox } from "../../components/Checkbox";
import { NumberField } from "../../components/NumberField";
import { Readout, ReadoutRow } from "../../components/Readout";
import { SettingRow, Settings } from "../../components/Settings";
import { attachSurface, type SurfaceView } from "../../gl/surface";
import { useDfStore } from "../../lib/df";
import type { RangeDopplerFrame } from "../../lib/frame";
import { surfaceHub } from "../../lib/surface";
import type { PassiveRadarParams, PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";
import { DEFAULT_ILLUMINATOR, DEFAULT_RADAR_PARAMS, dopplerAxisHz, rangeAxisKm } from "./radar";

export function RangeDopplerFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const state = useDfStore((store) => store.byNode[node.id]);
  if (node.kind !== "passive_radar") {
    return null;
  }
  const settings = node.data.settings ?? DEFAULT_RADAR_PARAMS;
  const update = (next: Partial<PassiveRadarParams>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "passive_radar"
          ? {
              ...current,
              data: { settings: { ...(current.data.settings ?? DEFAULT_RADAR_PARAMS), ...next } },
            }
          : current,
      ),
    }));
  };
  const detections = state?.detections ?? [];
  return (
    <NodeShell
      node={node}
      title="Passive radar"
      category="channel"
      subtitle={`${settings.cpi_ms} ms · ${settings.max_range_bins} range bins`}
      live={detections.length > 0}
    >
      <FaceBody scroll={false}>
        <RangeDopplerView node={node.id} />
        <Readout>
          <ReadoutRow label="Detections">{String(detections.length)}</ReadoutRow>
          {detections.slice(0, 3).map((hit) => (
            <ReadoutRow
              key={`${hit.range_bin}:${hit.doppler_hz}`}
              label={hit.track_id == null ? `Bin ${hit.range_bin}` : `Target ${hit.track_id}`}
            >
              {hit.range_km.toFixed(2)} km · {hit.doppler_hz >= 0 ? "+" : ""}
              {hit.doppler_hz.toFixed(1)} Hz · {hit.snr_db.toFixed(1)} dB
            </ReadoutRow>
          ))}
        </Readout>
        <RadarSettings settings={settings} onChange={update} />
      </FaceBody>
    </NodeShell>
  );
}

/// The surface on its own, so the desktop face and the field mission draw the same picture.
export function RangeDopplerView({ node }: { node: string }) {
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const view = useRef<SurfaceView | null>(null);
  const frame = useRef<RangeDopplerFrame | null>(surfaceHub.latest(node));
  const detections = useDfStore((store) => store.byNode[node]?.detections);

  useEffect(() => {
    const element = canvas.current;
    if (element === null) {
      return;
    }
    const attached = attachSurface(element);
    view.current = attached;
    return () => {
      attached.dispose();
      view.current = null;
    };
  }, []);

  useEffect(() => {
    return surfaceHub.subscribe(node, (next) => {
      frame.current = next;
      view.current?.draw(next, marksOf(next, detections));
    });
  }, [node, detections]);

  useEffect(() => {
    const latest = frame.current;
    if (latest !== null) {
      view.current?.draw(latest, marksOf(latest, detections));
    }
  }, [detections]);

  if (frame.current === null) {
    return <FaceEmpty>Waiting for the first coherent processing interval…</FaceEmpty>;
  }
  return (
    <div className="relative min-h-40 flex-1">
      <canvas ref={canvas} className="absolute inset-0 h-full w-full" />
    </div>
  );
}

function marksOf(
  frame: RangeDopplerFrame,
  detections: readonly { range_bin: number; doppler_hz: number }[] | undefined,
): { range: number; doppler: number }[] {
  if (detections === undefined || frame.dopplerStepHz === 0) {
    return [];
  }
  const centre = (frame.dopplers - 1) / 2;
  return detections.map((hit) => ({
    range: hit.range_bin,
    doppler: Math.round(centre + hit.doppler_hz / frame.dopplerStepHz),
  }));
}

function RadarSettings({
  settings,
  onChange,
}: {
  settings: PassiveRadarParams;
  onChange: (next: Partial<PassiveRadarParams>) => void;
}) {
  return (
    <Settings className="border-t border-line p-2">
      <SettingRow label="Integration" title={`${rangeAxisKm(settings).toFixed(1)} km of range`}>
        <NumberField
          label="Coherent processing interval in milliseconds"
          value={settings.cpi_ms}
          min={10}
          max={2_000}
          step={10}
          onCommit={(cpi_ms) => onChange({ cpi_ms })}
        />
      </SettingRow>
      <SettingRow label="Range bins">
        <NumberField
          label="Range bins"
          value={settings.max_range_bins}
          min={1}
          max={2_048}
          step={1}
          onCommit={(max_range_bins) => onChange({ max_range_bins })}
        />
      </SettingRow>
      <SettingRow label="Transmitter">
        <Checkbox
          label="The transmitter's place is known"
          checked={settings.illuminator !== null && settings.illuminator !== undefined}
          onChange={(known) => onChange({ illuminator: known ? DEFAULT_ILLUMINATOR : null })}
        />
      </SettingRow>
      {settings.illuminator !== null && settings.illuminator !== undefined && (
        <>
          <SettingRow label="Latitude">
            <NumberField
              label="Transmitter latitude in degrees"
              value={settings.illuminator.lat}
              min={-90}
              max={90}
              step={0.0001}
              onCommit={(lat) =>
                onChange({ illuminator: { ...(settings.illuminator ?? DEFAULT_ILLUMINATOR), lat } })
              }
            />
          </SettingRow>
          <SettingRow label="Longitude">
            <NumberField
              label="Transmitter longitude in degrees"
              value={settings.illuminator.lon}
              min={-180}
              max={180}
              step={0.0001}
              onCommit={(lon) =>
                onChange({ illuminator: { ...(settings.illuminator ?? DEFAULT_ILLUMINATOR), lon } })
              }
            />
          </SettingRow>
          <SettingRow label="Frequency">
            <NumberField
              label="Transmitter frequency in hertz"
              value={settings.illuminator.freq_hz}
              min={1}
              step={100_000}
              onCommit={(freq_hz) =>
                onChange({
                  illuminator: { ...(settings.illuminator ?? DEFAULT_ILLUMINATOR), freq_hz },
                })
              }
            />
          </SettingRow>
        </>
      )}
      <SettingRow
        label="Doppler span"
        title={`${dopplerAxisHz(settings).toFixed(1)} Hz either side of zero`}
      >
        <NumberField
          label="Doppler span in hertz"
          value={settings.doppler_span_hz}
          min={1}
          max={5_000}
          step={10}
          onCommit={(doppler_span_hz) => onChange({ doppler_span_hz })}
        />
      </SettingRow>
    </Settings>
  );
}

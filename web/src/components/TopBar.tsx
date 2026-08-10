// The top bar is the radio (DESIGN.md §5): the dial, the tune step, the receiver's own
// settings, and recording. Nothing that changes what you are *looking* at belongs here — that
// is the tab bar's job — and nothing else gets a row of its own.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { deleteDeviceSet, RECORDINGS_KEY, recordDeviceSet, STATE_KEY } from "../lib/api";
import type { Capabilities, DeviceSet, RecordAction, RecordingStatus } from "../lib/types";
import { useDevicePatch } from "../lib/useDevicePatch";
import { BTN, BTN_DANGER, BTN_QUIET, CHIP, ICON_BTN, segment } from "./controls";
import { formatStep, type Range, TUNE_STEPS_HZ } from "./dial";
import { FrequencyDial } from "./FrequencyDial";
import { OpenRadio } from "./OpenRadio";
import { Popover } from "./Popover";
import { RadioSettings } from "./RadioSettings";
import { deriveRecordControl, formatBytes, formatDuration, recordingElapsedS } from "./recordings";

/** The dial's limits. Discontiguous tuners report several ranges; the dial spans their envelope
 * and the server rejects a frequency that falls in a gap — which is the honest report, since
 * only the driver knows where its holes are. */
export function tuningRange(caps: Capabilities): Range {
  const ranges = caps.freq_ranges;
  if (ranges.length === 0) {
    return { min: 0, max: 6e9 };
  }
  return {
    min: Math.min(...ranges.map((r) => r.min)),
    max: Math.max(...ranges.map((r) => r.max)),
  };
}

export function TopBar({
  active,
  deviceSets,
  onSelect,
  connected,
  clients,
  stepHz,
  onStepHz,
}: {
  active: DeviceSet | null;
  deviceSets: readonly DeviceSet[];
  onSelect: (ds: number | null) => void;
  connected: boolean;
  clients: number;
  stepHz: number;
  onStepHz: (hz: number) => void;
}) {
  const { applyPatch, cachedSettings } = useDevicePatch();
  const centerHz = active?.settings.center_hz ?? 0;
  const range = active === null ? { min: 0, max: 6e9 } : tuningRange(active.capabilities);

  const tune = (hz: number): void => {
    if (active !== null) {
      applyPatch(active.id, { center_hz: hz });
    }
  };
  const nudge = (direction: number): void => {
    if (active === null) {
      return;
    }
    const from = cachedSettings(active.id)?.center_hz ?? centerHz;
    tune(Math.min(range.max, Math.max(range.min, from + direction * stepHz)));
  };

  return (
    // `overflow-hidden` is the phone guard: without it a bar that cannot fit widens the
    // document and every panel below it scrolls sideways.
    <header className="flex h-14 shrink-0 items-center gap-3 overflow-hidden border-b border-line bg-panel pr-2 pl-3">
      <span
        className="font-mono text-sm font-semibold tracking-tight text-accent max-md:hidden"
        title="sdr-- — software-defined radio"
      >
        sdr--
      </span>

      {active === null ? (
        <span className="text-sm text-ink-dim">No receiver open</span>
      ) : (
        <>
          <FrequencyDial hz={centerHz} range={range} onTune={tune} />
          <StepControl stepHz={stepHz} onStepHz={onStepHz} onNudge={nudge} />
        </>
      )}

      <div className="ml-auto flex min-w-0 items-center gap-2">
        {/* Only the live readout earns a place in the bar; starting and stopping is a receiver
            action, so it lives with the receiver's other controls. */}
        {active?.recording != null && (
          <RecordingReadout
            status={active.recording}
            sampleRate={active.settings.sample_rate ?? 0}
          />
        )}
        <RadioMenu active={active} deviceSets={deviceSets} onSelect={onSelect} />
        <LinkState connected={connected} clients={clients} />
      </div>
    </header>
  );
}

/** Step down / step ladder / step up. The middle button names the current step rather than
 * hiding it in a shortcut, so the arrow keys' effect is always visible. */
function StepControl({
  stepHz,
  onStepHz,
  onNudge,
}: {
  stepHz: number;
  onStepHz: (hz: number) => void;
  onNudge: (direction: number) => void;
}) {
  return (
    <div className="flex items-center overflow-hidden rounded-[3px] border border-line-strong bg-panel-2">
      <button
        type="button"
        className={ICON_BTN}
        onClick={() => onNudge(-1)}
        aria-label={`Tune down ${formatStep(stepHz)}`}
      >
        −
      </button>
      <Popover
        label={<span className="legend w-[4.5rem] text-center text-ink">{formatStep(stepHz)}</span>}
        triggerClass={`${BTN_QUIET} h-7 rounded-none border-x border-line-strong px-1`}
        width="w-32"
      >
        {(close) => (
          <div className="flex flex-col gap-0.5">
            {TUNE_STEPS_HZ.map((step) => (
              <button
                key={step}
                type="button"
                className={`${segment(step === stepHz)} justify-end tabular-nums`}
                onClick={() => {
                  onStepHz(step);
                  close();
                }}
              >
                {formatStep(step)}
              </button>
            ))}
          </div>
        )}
      </Popover>
      <button
        type="button"
        className={ICON_BTN}
        onClick={() => onNudge(1)}
        aria-label={`Tune up ${formatStep(stepHz)}`}
      >
        +
      </button>
    </div>
  );
}

/** The receiver itself: which one, how it is set up, and how to open or close one. */
function RadioMenu({
  active,
  deviceSets,
  onSelect,
}: {
  active: DeviceSet | null;
  deviceSets: readonly DeviceSet[];
  onSelect: (ds: number | null) => void;
}) {
  const queryClient = useQueryClient();
  const closeMut = useMutation({
    mutationFn: deleteDeviceSet,
    onSuccess: () => onSelect(null),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });
  const faulted = active?.status === "error";

  return (
    <Popover
      align="end"
      width="w-96"
      triggerClass={`${CHIP} max-w-64 max-md:max-w-28 ${faulted ? "border-danger text-danger" : ""}`}
      label={
        <>
          <span className="truncate">{active?.device.label ?? "Open receiver"}</span>
          {active !== null && (
            <span className={`legend shrink-0 ${faulted ? "text-danger" : "text-ok"}`}>
              {active.status}
            </span>
          )}
          <span aria-hidden className="text-ink-faint">
            ▾
          </span>
        </>
      }
    >
      {(close) => (
        <div className="flex flex-col gap-3">
          {active === null ? (
            <OpenRadio
              onOpened={(id) => {
                onSelect(id);
                close();
              }}
            />
          ) : (
            <>
              {faulted && (
                <p role="alert" className="font-mono text-xs text-danger">
                  Device fault · {active.error ?? "unknown error"}
                </p>
              )}
              <RadioSettings active={active} />

              <div className="flex items-center justify-between border-t border-line pt-3">
                <span className="legend">Capture</span>
                <RecordControl active={active} />
              </div>

              {deviceSets.length > 1 && (
                <div className="flex flex-col gap-1 border-t border-line pt-3">
                  <span className="legend">Open receivers</span>
                  {deviceSets.map((set) => (
                    <button
                      key={set.id}
                      type="button"
                      className={`${segment(set.id === active.id)} justify-start`}
                      onClick={() => {
                        onSelect(set.id);
                        close();
                      }}
                    >
                      {set.device.label}
                    </button>
                  ))}
                </div>
              )}

              <div className="flex justify-end border-t border-line pt-3">
                <button
                  type="button"
                  className={BTN_DANGER}
                  disabled={closeMut.isPending}
                  onClick={() => {
                    closeMut.mutate(active.id);
                    close();
                  }}
                >
                  Close receiver
                </button>
              </div>
            </>
          )}
        </div>
      )}
    </Popover>
  );
}

function RecordControl({ active }: { active: DeviceSet }) {
  const queryClient = useQueryClient();
  const record = deriveRecordControl(active);
  const recordMut = useMutation({
    mutationFn: (v: { ds: number; action: RecordAction }) => recordDeviceSet(v.ds, v.action),
    // Belt-and-braces: the server emits "recordings" (stop indexes a SigMF pair) and
    // "device_set", but a missed WS event must not strand the button.
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: STATE_KEY });
      void queryClient.invalidateQueries({ queryKey: RECORDINGS_KEY });
    },
  });

  if (record.kind === "recording") {
    return (
      <span className="flex items-center gap-2">
        <RecordingReadout status={record.status} sampleRate={active.settings.sample_rate ?? 0} />
        <button
          type="button"
          className={BTN}
          disabled={recordMut.isPending}
          onClick={() => recordMut.mutate({ ds: active.id, action: "stop" })}
        >
          Stop
        </button>
      </span>
    );
  }
  return (
    <button
      type="button"
      className={BTN}
      disabled={!record.canStart || recordMut.isPending}
      title={record.canStart ? "Record IQ to a SigMF pair" : "The receiver must be running"}
      onClick={() => recordMut.mutate({ ds: active.id, action: "start" })}
    >
      <span aria-hidden className="text-danger">
        ●
      </span>
      Record
    </button>
  );
}

// Child component so the 1 s elapsed tick re-renders only the readout, not the whole bar. Once
// the recording faulted the writer is dead, so the ticker stops — wall clock would overstate
// what was captured.
function RecordingReadout({ status, sampleRate }: { status: RecordingStatus; sampleRate: number }) {
  const faulted = status.error != null;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (faulted) {
      return;
    }
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [faulted]);
  return (
    <span className={`font-mono text-xs ${faulted ? "text-danger" : "text-ink"}`}>
      <span aria-hidden className="text-danger">
        ●{" "}
      </span>
      {formatDuration(recordingElapsedS(status, now, sampleRate))} · {formatBytes(status.bytes)}
      {status.overruns > 0 && ` · ${status.overruns} overruns`}
      {faulted && ` · ${status.error}`}
    </span>
  );
}

/** Link state is a status, not a control: a dot plus a word, so it is never carried by colour
 * alone. The client count only appears when someone else is driving the radio. */
function LinkState({ connected, clients }: { connected: boolean; clients: number }) {
  return (
    <span className="flex items-center gap-1.5 pl-1 text-xs text-ink-dim">
      {clients > 1 && <span className="font-mono">{clients} clients</span>}
      <span
        aria-hidden
        className={`inline-block size-2 rounded-full ${connected ? "bg-ok" : "bg-danger"}`}
      />
      <span className="max-md:sr-only">{connected ? "live" : "reconnecting…"}</span>
    </span>
  );
}

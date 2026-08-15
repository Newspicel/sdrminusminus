import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "../../components/BaseControls";
import { ALERT, BTN, BTN_DANGER, BTN_PRIMARY, CHIP, LABEL } from "../../components/controls";
import { formatHz } from "../../components/format";
import { Select } from "../../components/Select";
import { runTool } from "../../lib/api";
import type { NanoVnaCalibration, NanoVnaStandard, NanoVnaSweepState } from "../../lib/types";
import { type CalibrationStep, nanoVnaCalibrateRequest, nanoVnaCalibration } from "./nanovna";

/** The one-port standards, in the order the wizard walks them. Open, short and load are what a
 * reflection calibration needs; thru and isolation only matter once something is connected to
 * the second port. */
const ONE_PORT: ReadonlyArray<{
  standard: NanoVnaStandard;
  step: CalibrationStep;
  label: string;
  hint: string;
}> = [
  {
    standard: "open",
    step: { step: "open" },
    label: "Open",
    hint: "Leave CH0 unterminated, or fit the OPEN standard.",
  },
  {
    standard: "short",
    step: { step: "short" },
    label: "Short",
    hint: "Fit the SHORT standard to CH0.",
  },
  {
    standard: "load",
    step: { step: "load" },
    label: "Load",
    hint: "Fit the 50 Ω LOAD standard to CH0.",
  },
];

const TWO_PORT: ReadonlyArray<{
  standard: NanoVnaStandard;
  step: CalibrationStep;
  label: string;
  hint: string;
}> = [
  {
    standard: "isolation",
    step: { step: "isolation" },
    label: "Isolation",
    hint: "Leave CH1 terminated in 50 Ω, or open, with CH0 loaded.",
  },
  {
    standard: "thru",
    step: { step: "thru" },
    label: "Thru",
    hint: "Join CH0 to CH1 with the THRU adapter.",
  },
];

const SLOT_OPTIONS = [0, 1, 2, 3, 4, 5, 6].map((slot) => ({
  value: slot,
  label: `Slot ${slot}`,
}));

export function CalibrationPanel({
  port,
  range,
  state,
  onState,
}: {
  port: string;
  range: NanoVnaSweepState;
  state: NanoVnaCalibration | null;
  onState: (state: NanoVnaCalibration) => void;
}) {
  const [slot, setSlot] = useState(0);
  const [pending, setPending] = useState<string | null>(null);
  const calibrate = useMutation({
    mutationFn: async ({ step }: { step: CalibrationStep; key: string }) =>
      nanoVnaCalibration(
        await runTool(
          nanoVnaCalibrateRequest(port, step, step.step === "reset" ? range : undefined),
        ),
      ),
    onSettled: () => setPending(null),
    onSuccess: (next) => {
      if (next !== null) {
        onState(next);
      }
    },
  });

  function run(key: string, step: CalibrationStep) {
    setPending(key);
    calibrate.mutate({ step, key });
  }

  const measured = new Set(state?.standards ?? []);
  const busy = calibrate.isPending;
  const reflectionDone = ONE_PORT.every((entry) => measured.has(entry.standard));

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          className={BTN_PRIMARY}
          disabled={busy || port === ""}
          onClick={() => run("reset", { step: "reset" })}
        >
          {pending === "reset" ? "Starting…" : "Start over this range"}
        </Button>
        <span className="font-mono text-xs text-ink-dim">
          {formatHz(range.start_hz)} – {formatHz(range.stop_hz)}, {range.points} points
        </span>
      </div>

      <p className="text-xs leading-snug text-ink-dim">
        Calibration belongs to the range it was measured over. Start it here, fit each standard when
        the step asks for it, then apply — the instrument keeps the result until it is reset or
        another slot is recalled.
      </p>

      <StepGroup
        title="Reflection (CH0)"
        entries={ONE_PORT}
        measured={measured}
        busy={busy}
        pending={pending}
        onRun={run}
      />
      <StepGroup
        title="Transmission (CH0 → CH1), optional"
        entries={TWO_PORT}
        measured={measured}
        busy={busy}
        pending={pending}
        onRun={run}
      />

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          className={BTN_PRIMARY}
          disabled={busy || !reflectionDone}
          onClick={() => run("finish", { step: "finish" })}
          title={
            reflectionDone
              ? "Compute the error terms and switch correction on"
              : "Measure open, short and load first"
          }
        >
          {pending === "finish" ? "Applying…" : "Apply calibration"}
        </Button>
        <Button
          type="button"
          className={BTN}
          disabled={busy}
          onClick={() => run("toggle", { step: state?.applied === true ? "disable" : "enable" })}
        >
          {state?.applied === true ? "Switch correction off" : "Switch correction on"}
        </Button>
      </div>

      <div className="flex flex-wrap items-end gap-2">
        <div className="flex flex-col gap-1">
          <span className={LABEL}>Storage</span>
          <Select
            label="Calibration slot"
            value={slot}
            options={SLOT_OPTIONS}
            onChange={setSlot}
            className="w-32"
          />
        </div>
        <Button
          type="button"
          className={BTN}
          disabled={busy}
          onClick={() => run("save", { step: "save", slot })}
        >
          {pending === "save" ? "Saving…" : "Save to slot"}
        </Button>
        <Button
          type="button"
          className={BTN}
          disabled={busy}
          onClick={() => run("recall", { step: "recall", slot })}
        >
          {pending === "recall" ? "Recalling…" : "Recall slot"}
        </Button>
        <Button
          type="button"
          className={BTN_DANGER}
          disabled={busy}
          onClick={() => run("reset", { step: "reset" })}
        >
          Clear
        </Button>
      </div>

      {calibrate.isError && <p className={ALERT}>{calibrate.error.message}</p>}
      <CalibrationState state={state} />
    </div>
  );
}

function StepGroup({
  title,
  entries,
  measured,
  busy,
  pending,
  onRun,
}: {
  title: string;
  entries: ReadonlyArray<{
    standard: NanoVnaStandard;
    step: CalibrationStep;
    label: string;
    hint: string;
  }>;
  measured: ReadonlySet<NanoVnaStandard>;
  busy: boolean;
  pending: string | null;
  onRun: (key: string, step: CalibrationStep) => void;
}) {
  return (
    <section className="flex flex-col gap-1.5">
      <h4 className={LABEL}>{title}</h4>
      <ul className="flex flex-col gap-1.5">
        {entries.map((entry) => {
          const done = measured.has(entry.standard);
          return (
            <li key={entry.standard} className="flex items-center gap-2">
              <Button
                type="button"
                className={`${BTN} w-28 justify-center`}
                disabled={busy}
                onClick={() => onRun(entry.standard, entry.step)}
              >
                {pending === entry.standard ? "Measuring…" : entry.label}
              </Button>
              <span
                aria-label={done ? `${entry.label} measured` : `${entry.label} not measured`}
                className={`font-mono text-xs ${done ? "text-ok" : "text-ink-faint"}`}
              >
                {done ? "✓" : "○"}
              </span>
              <span className="min-w-0 text-xs text-ink-dim">{entry.hint}</span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function CalibrationState({ state }: { state: NanoVnaCalibration | null }) {
  if (state === null) {
    return null;
  }
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className={CHIP}>
        <span className="text-ink-faint">correction</span>
        <span className={state.applied ? "text-ok" : "text-ink-dim"}>
          {state.applied ? "on" : "off"}
        </span>
      </span>
      <span className={CHIP}>
        <span className="text-ink-faint">standards</span>
        {state.standards.length === 0 ? "none" : state.standards.join(" ")}
      </span>
      <span className={CHIP}>
        <span className="text-ink-faint">error terms</span>
        {state.error_terms.length === 0 ? "none" : state.error_terms.join(" ")}
      </span>
    </div>
  );
}

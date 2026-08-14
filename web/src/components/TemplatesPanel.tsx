import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { applyTemplate, STATE_KEY, templatesQuery } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, TemplateInfo } from "../lib/types";
import { BTN } from "./controls";
import { deviceId } from "./OpenRadio";

/** Whether this radio is one the server said can run the template.
 *
 * The rule itself is `TemplateInfo::unmet_by` in `crates/wire`, evaluated server-side against
 * every probed radio's profile — frequency span, sample rate and whether it has the direction
 * the template needs. This is the lookup, not a second copy of the rule: the engine's rejection
 * on apply is still the authority, but a card that says why beats a failed apply. */
export function supports(template: TemplateInfo, set: DeviceSet | null): boolean {
  return set !== null && (template.supported_devices ?? []).includes(deviceId(set.device));
}

export function TemplatesPanel({
  active,
  onApplied,
}: {
  active: DeviceSet | null;
  onApplied?: (template: TemplateInfo) => void;
}) {
  const queryClient = useQueryClient();
  const templates = useQuery(templatesQuery());
  const [applied, setApplied] = useState<TemplateInfo | null>(null);

  const applyMut = useMutation({
    mutationFn: (v: { template: TemplateInfo; ds: number }) => applyTemplate(v.template.id, v.ds),
    onSuccess: (_void, v) => {
      setApplied(v.template);
      onApplied?.(v.template);
    },
    onError: (e) => pushToast(e.message),
    onSettled: () => void queryClient.invalidateQueries({ queryKey: STATE_KEY }),
  });

  return (
    <div className="flex flex-col gap-2 p-3">
      {applied !== null && (
        <div className="rounded border border-accent bg-accent/10 px-3 py-2 text-sm text-ink">
          <div className="font-semibold text-accent">{applied.name}</div>
          <p className="mt-1 text-ink-dim">{applied.explainer}</p>
        </div>
      )}

      {active === null && (
        <span className="text-sm text-ink-dim">
          A template configures one radio: select the device node it should land on.
        </span>
      )}

      <div className="grid gap-2 sm:grid-cols-2">
        {(templates.data?.templates ?? []).map((t) => {
          const ok = supports(t, active);
          return (
            <div
              key={t.id}
              className="flex flex-col gap-1 rounded-[3px] border border-line bg-panel px-3 py-2"
            >
              <div className="text-sm font-semibold text-ink">{t.name}</div>
              <div className="text-xs text-ink-dim">{t.description}</div>
              <div className="font-mono text-[10px] text-ink-dim">
                {(t.center_hz / 1e6).toFixed(3)} MHz · {(t.sample_rate / 1e6).toFixed(3)} Msps ·{" "}
                {t.channels.length} channel{t.channels.length === 1 ? "" : "s"}
              </div>
              {/* Applying a template retunes one radio and replaces its channels, and that is
                  not undoable — so the button names the radio it is about to do it to, rather
                  than the drawer stating a target for sections that never had one. */}
              <button
                type="button"
                className={`${BTN} mt-1 max-w-full self-start`}
                disabled={!active || !ok || applyMut.isPending}
                title={
                  active === null || ok
                    ? undefined
                    : `${active.device.label} cannot run this template`
                }
                onClick={() => active && applyMut.mutate({ template: t, ds: active.id })}
              >
                <span className="truncate">
                  {active === null
                    ? "Apply"
                    : ok
                      ? `Apply to ${active.device.label}`
                      : `${active.device.label} cannot run this`}
                </span>
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

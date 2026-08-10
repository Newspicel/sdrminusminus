// Template gallery (PLAN §10, M5): one click configures device + channels for a known
// activity, and the explainer says what the user is now looking at.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { applyTemplate, STATE_KEY, templatesQuery } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, TemplateInfo } from "../lib/types";
import { BTN } from "./controls";

/** Whether the open device can tune what the template needs. Reported instead of enforced —
 * the capability list is what the device advertises, and the engine's rejection is the real
 * authority — but a greyed-out card beats a failed apply. */
export function reachable(template: TemplateInfo, set: DeviceSet | null): boolean {
  const ranges = set?.capabilities.freq_ranges ?? [];
  if (ranges.length === 0) {
    return true;
  }
  return ranges.some((r) => template.min_freq_hz >= r.min && template.max_freq_hz <= r.max);
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

      <div className="grid gap-2 sm:grid-cols-2">
        {(templates.data?.templates ?? []).map((t) => {
          const ok = reachable(t, active);
          return (
            <div
              key={t.id}
              className="flex flex-col gap-1 rounded border border-line bg-panel-2 px-3 py-2"
            >
              <div className="text-sm font-semibold text-ink">{t.name}</div>
              <div className="text-xs text-ink-dim">{t.description}</div>
              <div className="font-mono text-[10px] text-ink-dim">
                {(t.center_hz / 1e6).toFixed(3)} MHz · {(t.sample_rate / 1e6).toFixed(3)} Msps ·{" "}
                {t.channels.length} channel{t.channels.length === 1 ? "" : "s"}
              </div>
              <button
                type="button"
                className={`${BTN} mt-1 self-start`}
                disabled={!active || !ok || applyMut.isPending}
                title={ok ? undefined : "This device cannot tune that frequency"}
                onClick={() => active && applyMut.mutate({ template: t, ds: active.id })}
              >
                {ok ? "Apply" : "Out of range"}
              </button>
            </div>
          );
        })}
      </div>
      {!active && <span className="text-sm text-ink-dim">Open a device to apply a template.</span>}
    </div>
  );
}

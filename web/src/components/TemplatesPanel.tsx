import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Info } from "lucide-react";
import { useState } from "react";
import { applyTemplate, STATE_KEY, templatesQuery } from "../lib/api";
import { pushToast } from "../lib/toasts";
import type { DeviceSet, TemplateInfo } from "../lib/types";
import { Button } from "./BaseControls";
import { BTN_SM, ICON_BTN_SM } from "./controls";
import { formatHz, formatSampleRate } from "./format";
import { Icon } from "./Icon";
import { List, ListRow, Panel, PanelHint } from "./ListPanel";
import { Popover } from "./Popover";
import { supports } from "./templates";

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
    <Panel>
      {applied !== null && (
        <div className="rounded-[3px] border border-accent-dim bg-accent/10 px-3 py-2">
          <div className="font-mono text-xs text-accent">{applied.name}</div>
          <p className="mt-1 text-xs text-ink-dim">{applied.explainer}</p>
        </div>
      )}
      {active === null && <PanelHint>Select a device node to apply a template.</PanelHint>}
      <List>
        {(templates.data?.templates ?? []).map((t) => {
          const ok = supports(t, active);
          return (
            <ListRow
              key={t.id}
              primary={t.name}
              badge={
                <span className="legend shrink-0 tabular-nums">
                  {formatHz(t.center_hz)} · {formatSampleRate(t.sample_rate)} · {t.channels.length}{" "}
                  ch
                </span>
              }
              secondary={t.description}
              actions={
                <>
                  <Popover
                    label={<Icon glyph={Info} size={12} />}
                    triggerClass={ICON_BTN_SM}
                    title={`About ${t.name}`}
                    width="w-72"
                    align="end"
                    openOnHover
                  >
                    {() => <p className="text-xs text-ink-dim">{t.explainer}</p>}
                  </Popover>
                  <Button
                    type="button"
                    className={BTN_SM}
                    disabled={!active || !ok || applyMut.isPending}
                    title={
                      active === null
                        ? undefined
                        : ok
                          ? `Apply to ${active.device.label}`
                          : `${active.device.label} cannot run this template`
                    }
                    onClick={() => active && applyMut.mutate({ template: t, ds: active.id })}
                  >
                    Apply
                  </Button>
                </>
              }
            />
          );
        })}
      </List>
    </Panel>
  );
}

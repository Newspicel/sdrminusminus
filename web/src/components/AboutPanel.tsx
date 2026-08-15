import { Collapsible } from "@base-ui/react/collapsible";
import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { aboutQuery, licenseTextQuery } from "../lib/api";
import type { Attribution } from "../lib/types";
import { groupComponents, licenseSummary, notedComponents } from "./about";
import { Button, Input } from "./BaseControls";
import { BTN, BTN_QUIET, FIELD, SURFACE } from "./controls";

export function AboutPanel({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [query, setQuery] = useState("");
  const [textId, setTextId] = useState<string | null>(null);
  const about = useQuery(aboutQuery(open));

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next);
        if (!next) setTextId(null);
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex max-h-[85vh] w-full max-w-3xl -translate-x-1/2 -translate-y-1/2 flex-col p-4`}
        >
          <div className="flex shrink-0 items-baseline justify-between gap-4">
            <Dialog.Title className="text-base font-medium text-ink">
              sdr-- {about.data?.version ?? ""}
            </Dialog.Title>
            <Dialog.Description className="legend">
              {about.data ? `${about.data.license} licensed` : "Loading…"}
            </Dialog.Description>
          </div>

          {about.isError && <p className="mt-3 text-xs text-danger">Could not load the notices.</p>}

          {about.data && (
            <div className="mt-2 min-h-0 flex-1 overflow-auto">
              <p className="text-xs text-ink-dim">
                <a
                  href={about.data.repository}
                  target="_blank"
                  rel="noreferrer"
                  className="text-accent hover:underline"
                >
                  {about.data.repository}
                </a>
              </p>

              <Collapsible.Root className="mt-3">
                <Collapsible.Trigger className="cursor-pointer text-xs text-ink-dim hover:text-ink">
                  License
                </Collapsible.Trigger>
                <Collapsible.Panel>
                  <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-[3px] bg-panel-2 p-2 font-mono text-[11px] text-ink-dim">
                    {about.data.license_text}
                  </pre>
                </Collapsible.Panel>
              </Collapsible.Root>

              <Noted components={about.data.components} onOpenText={setTextId} />

              <div className="mt-4 flex items-baseline justify-between gap-4">
                <h3 className="text-xs font-medium text-ink">
                  Third-party components ({about.data.components.length})
                </h3>
                <Input
                  className={`${FIELD} w-48`}
                  type="search"
                  name="component-filter"
                  placeholder="Filter by name or license"
                  aria-label="Filter third-party components"
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                />
              </div>

              <p className="legend mt-1">
                {licenseSummary(about.data.components)
                  .slice(0, 4)
                  .map((entry) => `${entry.count} × ${entry.license}`)
                  .join(" · ")}
              </p>

              <Components components={about.data.components} query={query} onOpenText={setTextId} />
            </div>
          )}

          <div className="mt-4 flex shrink-0 justify-end">
            <Dialog.Close className={BTN}>Close</Dialog.Close>
          </div>

          {textId !== null && <LicenseText id={textId} onClose={() => setTextId(null)} />}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Noted({
  components,
  onOpenText,
}: {
  components: Attribution[];
  onOpenText: (id: string) => void;
}) {
  const noted = notedComponents(components);
  if (noted.length === 0) return null;
  return (
    <section className="mt-3">
      <h3 className="text-xs font-medium text-ink">Worth knowing</h3>
      <ul className="mt-1.5 space-y-2">
        {noted.map((component) => (
          <li key={`${component.source}:${component.name}`} className="text-xs">
            <Row component={component} onOpenText={onOpenText} />
            <p className="mt-0.5 text-ink-dim">{component.note}</p>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Components({
  components,
  query,
  onOpenText,
}: {
  components: Attribution[];
  query: string;
  onOpenText: (id: string) => void;
}) {
  const groups = groupComponents(components, query);
  if (groups.length === 0) {
    return <p className="mt-2 text-xs text-ink-dim">Nothing matches “{query}”.</p>;
  }
  return (
    <div className="mt-1.5">
      {groups.map((group) => (
        <section key={group.source} className="mb-3">
          <h4 className="legend sticky top-0 bg-panel-3 py-1">
            {group.label} ({group.components.length})
          </h4>
          <ul>
            {group.components.map((component) => (
              <li key={`${component.source}:${component.name}:${component.version ?? ""}`}>
                <Row component={component} onOpenText={onOpenText} />
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

function Row({
  component,
  onOpenText,
}: {
  component: Attribution;
  onOpenText: (id: string) => void;
}) {
  return (
    <div className="flex flex-wrap items-baseline gap-x-2 py-0.5 text-xs">
      {component.url ? (
        <a
          href={component.url}
          target="_blank"
          rel="noreferrer"
          className="text-ink hover:text-accent hover:underline"
        >
          {component.name}
        </a>
      ) : (
        <span className="text-ink">{component.name}</span>
      )}
      {component.version !== undefined && component.version !== null && (
        <span className="font-mono text-[11px] text-ink-dim">{component.version}</span>
      )}
      <span className="text-ink-dim">{component.license}</span>
      {component.texts.map((id, index) => (
        <Button
          key={id}
          type="button"
          className={BTN_QUIET}
          onClick={() => onOpenText(id)}
          aria-label={`Read license text ${index + 1} for ${component.name}`}
        >
          text{component.texts.length > 1 ? ` ${index + 1}` : ""}
        </Button>
      ))}
    </div>
  );
}

function LicenseText({ id, onClose }: { id: string; onClose: () => void }) {
  const text = useQuery(licenseTextQuery(id));
  return (
    <div className="absolute inset-0 z-10 flex flex-col rounded-md bg-panel-3 p-4">
      <div className="flex items-baseline justify-between gap-4">
        <h3 className="text-sm font-medium text-ink">License text</h3>
        <Button type="button" className={BTN} onClick={onClose}>
          Back
        </Button>
      </div>
      <pre className="mt-3 min-h-0 flex-1 overflow-auto whitespace-pre-wrap font-mono text-[11px] text-ink-dim">
        {text.isError ? "Could not load this license text." : (text.data?.text ?? "Loading…")}
      </pre>
    </div>
  );
}

// The About dialog: what this build is, and what it is built out of.
//
// Every license here is delivered, not linked to — the texts are compiled into the server, so
// the panel works on a machine that has never had a network. That is the whole point of putting
// the notices in the product rather than in a file in the repository.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { aboutQuery, licenseTextQuery } from "../lib/api";
import type { Attribution } from "../lib/types";
import { groupComponents, licenseSummary, notedComponents } from "./about";
import { EmptyState } from "./EmptyState";

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
    <Dialog
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next);
        if (!next) setTextId(null);
      }}
    >
      <DialogContent className="flex max-h-[85vh] max-w-3xl flex-col" showCloseButton={false}>
        <div className="flex shrink-0 items-baseline justify-between gap-4">
          <DialogTitle className="text-base font-medium text-foreground">
            sdr-- {about.data?.version ?? ""}
          </DialogTitle>
          <DialogDescription className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70">
            {about.data ? `${about.data.license} licensed` : "Loading…"}
          </DialogDescription>
        </div>

        {about.isError && (
          <p className="mt-3 text-xs text-destructive">Could not load the notices.</p>
        )}

        {/* One scroll region for the whole body. The noted components alone run past a
              viewport, so scrolling only the list below them would leave the list clipped off
              the bottom of the dialog and the reader with no way to reach it. */}
        {about.data && (
          <ScrollArea className="mt-2 min-h-0 flex-1">
            <p className="text-xs text-muted-foreground">
              <a
                href={about.data.repository}
                target="_blank"
                rel="noreferrer"
                className="text-primary hover:underline"
              >
                {about.data.repository}
              </a>
            </p>

            <Collapsible className="mt-3">
              <CollapsibleTrigger className="cursor-pointer text-xs text-muted-foreground hover:text-foreground">
                License
              </CollapsibleTrigger>
              <CollapsibleContent>
                <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-[3px] bg-muted p-2 font-mono text-[11px] text-muted-foreground">
                  {about.data.license_text}
                </pre>
              </CollapsibleContent>
            </Collapsible>

            <Noted components={about.data.components} onOpenText={setTextId} />

            <div className="mt-4 flex items-baseline justify-between gap-4">
              <h3 className="text-xs font-medium text-foreground">
                Third-party components ({about.data.components.length})
              </h3>
              <Input
                className="w-48"
                type="search"
                name="component-filter"
                placeholder="Filter by name or license"
                aria-label="Filter third-party components"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
              />
            </div>

            <p className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70 mt-1">
              {licenseSummary(about.data.components)
                .slice(0, 4)
                .map((entry) => `${entry.count} × ${entry.license}`)
                .join(" · ")}
            </p>

            <Components components={about.data.components} query={query} onOpenText={setTextId} />
          </ScrollArea>
        )}

        <DialogFooter className="mt-4 shrink-0">
          <DialogClose render={<Button variant="outline" size="sm" />}>Close</DialogClose>
        </DialogFooter>

        {textId !== null && <LicenseText id={textId} onClose={() => setTextId(null)} />}
      </DialogContent>
    </Dialog>
  );
}

/** The components whose SPDX id is not the whole story, above the alphabetical bulk. A reader
 * who opens this panel to check for copyleft should not have to search for it. */
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
      <h3 className="text-xs font-medium text-foreground">Worth knowing</h3>
      <ul className="mt-1.5 space-y-2">
        {noted.map((component) => (
          <li key={`${component.source}:${component.name}`} className="text-xs">
            <Row component={component} onOpenText={onOpenText} />
            <p className="mt-0.5 text-muted-foreground">{component.note}</p>
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
    return <EmptyState>Nothing matches “{query}”.</EmptyState>;
  }
  return (
    <div className="mt-1.5">
      {groups.map((group) => (
        <section key={group.source} className="mb-3">
          <h4 className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70 sticky top-0 bg-popover py-1">
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
          className="text-foreground hover:text-primary hover:underline"
        >
          {component.name}
        </a>
      ) : (
        <span className="text-foreground">{component.name}</span>
      )}
      {component.version !== undefined && component.version !== null && (
        <span className="font-mono text-[11px] text-muted-foreground">{component.version}</span>
      )}
      <span className="text-muted-foreground">{component.license}</span>
      {component.texts.map((id, index) => (
        <Button
          key={id}
          type="button"
          variant="ghost"
          size="sm"
          onClick={() => onOpenText(id)}
          aria-label={`Read license text ${index + 1} for ${component.name}`}
        >
          text{component.texts.length > 1 ? ` ${index + 1}` : ""}
        </Button>
      ))}
    </div>
  );
}

/** One license text, fetched on demand. Nested inside the dialog rather than replacing it, so
 * closing it returns the reader to the row they were on. */
function LicenseText({ id, onClose }: { id: string; onClose: () => void }) {
  const text = useQuery(licenseTextQuery(id));
  return (
    <div className="absolute inset-0 z-10 flex flex-col rounded-md bg-popover p-4">
      <div className="flex items-baseline justify-between gap-4">
        <h3 className="text-sm font-medium text-foreground">License text</h3>
        <Button type="button" variant="outline" size="sm" onClick={onClose}>
          Back
        </Button>
      </div>
      <pre className="mt-3 min-h-0 flex-1 overflow-auto whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">
        {text.isError ? "Could not load this license text." : (text.data?.text ?? "Loading…")}
      </pre>
    </div>
  );
}

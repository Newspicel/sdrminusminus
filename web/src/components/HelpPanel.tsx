import { Dialog } from "@base-ui/react/dialog";
import { useQuery } from "@tanstack/react-query";
import {
  AudioWaveform,
  BookOpen,
  Braces,
  Bug,
  Code,
  LifeBuoy,
  type LucideIcon,
  MessagesSquare,
  Radio,
  Scale,
  Tag,
  Workflow,
} from "lucide-react";
import type { ReactNode } from "react";
import { BINDINGS, type Binding } from "../canvas/useHotkeys";
import { aboutQuery } from "../lib/api";
import { Button } from "./BaseControls";
import { BTN, DIALOG_TITLE, SURFACE } from "./controls";
import { DISCORD, docsPage, groupBindings, host, isApple, keyCaps } from "./help";
import { Icon } from "./Icon";

const ROW =
  "group flex h-7 w-full items-center gap-2 rounded-[3px] px-2 text-left text-xs text-ink " +
  "transition-colors duration-100 hover:bg-panel-2 aria-disabled:pointer-events-none aria-disabled:opacity-45";

const GROUPS = groupBindings(BINDINGS);

export function HelpPanel({
  open,
  onOpenChange,
  onShowAbout,
  onShowReport,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onShowAbout: () => void;
  onShowReport: () => void;
}) {
  const about = useQuery(aboutQuery(open));
  const repository = about.data?.repository ?? "";
  const handOff = (show: () => void) => () => {
    onOpenChange(false);
    show();
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex max-h-[85vh] w-[calc(100%-2rem)] max-w-[56rem] -translate-x-1/2 -translate-y-1/2 flex-col`}
        >
          <div className="flex shrink-0 items-baseline justify-between gap-4 px-4 pt-4 pb-3">
            <Dialog.Title className={DIALOG_TITLE}>Help</Dialog.Title>
            <Dialog.Description className="legend">
              {about.data ? `SDR-- ${about.data.version}` : ""}
            </Dialog.Description>
          </div>

          <div className="grid min-h-0 flex-1 overflow-auto border-y border-line md:grid-cols-[14rem_1fr]">
            <nav aria-label="Help links" className="flex flex-col gap-4 p-2 pt-3">
              <Section title="Learn">
                <LinkRow glyph={BookOpen} label="Guide" href={docsPage("")} />
                <LinkRow
                  glyph={Radio}
                  label="First receiver"
                  href={docsPage("getting-started/first-receiver")}
                />
                <LinkRow
                  glyph={Workflow}
                  label="Nodes and wires"
                  href={docsPage("getting-started/workspace")}
                />
                <LinkRow
                  glyph={AudioWaveform}
                  label="Decoders"
                  href={docsPage("user-guide/decoders")}
                />
                <LinkRow
                  glyph={Braces}
                  label="API"
                  href={new URL("/api/docs", window.location.href).href}
                />
              </Section>
              <Section title="Get help">
                <LinkRow
                  glyph={LifeBuoy}
                  label="Troubleshooting"
                  href={docsPage("troubleshooting")}
                />
                <LinkRow glyph={MessagesSquare} label="Ask on Discord" href={DISCORD} />
                <ActionRow glyph={Bug} label="Report a problem" onClick={handOff(onShowReport)} />
              </Section>
              <Section title="Project">
                <LinkRow glyph={Code} label="Source code" href={repository} />
                <LinkRow
                  glyph={Tag}
                  label="Releases"
                  href={repository && `${repository}/releases`}
                />
                <ActionRow glyph={Scale} label="Licenses" onClick={handOff(onShowAbout)} />
              </Section>
            </nav>
            <Keyboard />
          </div>

          <div className="flex shrink-0 justify-end px-4 py-3">
            <Dialog.Close className={BTN}>Close</Dialog.Close>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <h3 className="legend px-2 pb-1">{title}</h3>
      <ul>{children}</ul>
    </section>
  );
}

function LinkRow({ glyph, label, href }: { glyph: LucideIcon; label: string; href: string }) {
  const ready = href.length > 0;
  return (
    <li>
      <a
        href={ready ? href : undefined}
        aria-disabled={!ready || undefined}
        target="_blank"
        rel="noreferrer"
        className={ROW}
      >
        <RowLabel glyph={glyph} label={label} />
        <span className="legend truncate group-hover:text-ink-dim">{host(href)}</span>
      </a>
    </li>
  );
}

function ActionRow({
  glyph,
  label,
  onClick,
}: {
  glyph: LucideIcon;
  label: string;
  onClick: () => void;
}) {
  return (
    <li>
      <Button type="button" className={ROW} onClick={onClick}>
        <RowLabel glyph={glyph} label={label} />
      </Button>
    </li>
  );
}

function RowLabel({ glyph, label }: { glyph: LucideIcon; label: string }) {
  return (
    <>
      <span className="text-ink-faint group-hover:text-accent">
        <Icon glyph={glyph} />
      </span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </>
  );
}

function Keyboard() {
  const apple = isApple(navigator.userAgent);
  return (
    <section aria-labelledby="help-keyboard" className="bg-well p-4 md:border-l md:border-line">
      <div className="flex items-baseline justify-between gap-4">
        <h3 id="help-keyboard" className="legend">
          Keyboard
        </h3>
        <a
          href={docsPage("user-guide/keyboard")}
          target="_blank"
          rel="noreferrer"
          className="legend hover:text-accent"
        >
          All keys
        </a>
      </div>
      <div className="mt-3 gap-x-8 lg:columns-2">
        {GROUPS.map(({ group, bindings }) => (
          <div key={group} className="mb-4 break-inside-avoid">
            <h4 className="text-[11px] font-medium text-ink-dim">{group}</h4>
            <dl className="mt-1.5 grid grid-cols-[6.5rem_1fr] items-center gap-x-3 gap-y-1.5">
              {bindings.map((binding) => (
                <KeyRow key={binding.keys} binding={binding} apple={apple} />
              ))}
            </dl>
          </div>
        ))}
      </div>
    </section>
  );
}

function KeyRow({ binding, apple }: { binding: Binding; apple: boolean }) {
  return (
    <div className="contents">
      <dt className="flex flex-wrap items-center justify-end gap-1">
        {keyCaps(binding.keys, apple).map((part, index) =>
          part.cap ? (
            <kbd
              key={`${index}-${part.text}`}
              className="inline-flex h-5 min-w-5 items-center justify-center rounded-[3px] border border-line border-b-2 border-b-line-strong bg-panel-3 px-1 font-mono text-[10.5px] leading-none text-ink"
            >
              {part.text}
            </kbd>
          ) : (
            <span key={`${index}-${part.text}`} className="font-mono text-[10.5px] text-ink-faint">
              {part.text}
            </span>
          ),
        )}
      </dt>
      <dd className="text-xs text-ink-dim">{binding.what}</dd>
    </div>
  );
}

// The panel registry: one entry per `PanelKind` (PLAN §10, M6). The dock addresses components
// by the kind string, which is exactly what the persisted layout stores — adding a panel type
// means one wire variant and one entry here.
import type { IDockviewPanelProps } from "dockview-react";
import type { ReactNode } from "react";
import { BookmarksPanel } from "../components/BookmarksPanel";
import { ChannelsPanel } from "../components/ChannelsPanel";
import { DecoderLogPanel } from "../components/DecoderLogPanel";
import { AprsView, PagerView, RdsView, TargetsView, TextView } from "../components/DecoderPanels";
import { MapPanel } from "../components/MapPanel";
import { PresetsPanel } from "../components/PresetsPanel";
import { RecordingsPanel } from "../components/RecordingsPanel";
import { ScannerPanel } from "../components/ScannerPanel";
import { SpectrumDisplay } from "../components/SpectrumDisplay";
import { TemplatesPanel } from "../components/TemplatesPanel";
import type { ChannelInfo, PanelKind } from "../lib/types";
import { useShell } from "./context";

/// Channel types that emit decoder events. WFM is here because it carries RDS; the descriptor's
/// `decoder_kind` says so, but the view choice is per channel type.
const DECODER_KINDS = new Set(["wfm", "pocsag", "adsb", "ais", "aprs", "rtty", "morse"]);

/** Every kind, in the order an "add panel" menu offers them. Keeping the list here (rather than
 * deriving it from the generated union, which erases at runtime) is what lets the reverse mapper
 * reject a component name that is not a panel. */
export const PANEL_KINDS = [
  "spectrum",
  "channels",
  "decoders",
  "map",
  "scanner",
  "decoder_log",
  "presets",
  "bookmarks",
  "templates",
  "recordings",
] as const satisfies readonly PanelKind[];

const TITLES: Record<PanelKind, string> = {
  spectrum: "Spectrum",
  channels: "Channels",
  decoders: "Decoders",
  map: "Map",
  scanner: "Scanner",
  decoder_log: "Decoder log",
  presets: "Presets",
  bookmarks: "Bookmarks",
  templates: "Templates",
  recordings: "Recordings",
};

export function panelTitle(kind: PanelKind): string {
  return TITLES[kind];
}

/** Panel ids follow the layout the server authors for defaults and templates, so a panel added
 * here and one authored in Rust are the same panel. */
export function panelId(kind: PanelKind): string {
  return `panel:${kind}`;
}

/** A panel body that fills its dock rectangle and scrolls its own overflow. Every panel needs
 * one: the fixed M0–M5 layout supplied the scroll container, and a dock does not. */
function Body({ children, scroll = true }: { children: ReactNode; scroll?: boolean }) {
  return (
    <div className={`flex h-full min-h-0 flex-col ${scroll ? "overflow-y-auto" : ""}`}>
      {children}
    </div>
  );
}

/** Shown instead of a device-bound panel when no radio is open — a dock panel cannot be
 * conditionally unmounted the way the fixed layout's `{active && …}` did. */
function NoDevice({ what }: { what: string }) {
  return (
    <Body>
      <p className="px-4 py-3 text-sm text-ink-dim">Open a device to {what}.</p>
    </Body>
  );
}

function SpectrumPanel() {
  const shell = useShell();
  return (
    <Body scroll={false}>
      <SpectrumDisplay
        socket={shell.socket}
        deviceSet={shell.active?.id ?? null}
        connected={shell.connected}
        channels={shell.active?.channels ?? []}
        selectedChannel={shell.selectedChannel}
        onSelectChannel={shell.setSelectedChannel}
      />
    </Body>
  );
}

function ChannelsDockPanel() {
  const shell = useShell();
  if (shell.active === null) {
    return <NoDevice what="add channels" />;
  }
  return (
    <Body>
      <ChannelsPanel
        socket={shell.socket}
        deviceSet={shell.active}
        selected={shell.selectedChannel}
        onSelect={shell.setSelectedChannel}
      />
    </Body>
  );
}

/** One view per decoder channel on the active set. Per-channel *dock* panels would need stable
 * channel identity across restarts, which the engine does not have (M6 gap, PROGRESS): the
 * aggregate keeps every decoder reachable without persisting an id that would go stale. */
function DecodersPanel() {
  const shell = useShell();
  const channels = (shell.active?.channels ?? []).filter((c) => DECODER_KINDS.has(kindOf(c)));
  if (channels.length === 0) {
    return (
      <Body>
        <p className="px-4 py-3 text-sm text-ink-dim">
          {shell.active === null
            ? "Open a device and add a decoder channel."
            : "No decoder channel on this device set yet."}
        </p>
      </Body>
    );
  }
  const deviceSet = shell.active?.id ?? 0;
  return (
    <Body>
      {channels.map((channel) => {
        const kind = kindOf(channel);
        // Channel ids are allocated per device set, so two sets both have a channel 1. Scoping
        // on the id alone would pour one set's frames into the other's view.
        const scope = { deviceSet, channel: channel.id };
        const selected = shell.selectedChannel === channel.id;
        return (
          <section key={channel.id} className="border-b border-line last:border-b-0">
            <button
              type="button"
              onClick={() => shell.setSelectedChannel(channel.id)}
              className={`flex min-h-10 w-full items-center border-b border-line bg-panel px-4 text-left text-xs font-semibold uppercase tracking-wider ${
                selected ? "text-accent" : "text-ink-dim"
              }`}
            >
              {kind.toUpperCase()} · channel {channel.id}
            </button>
            {kind === "wfm" && <RdsView scope={scope} />}
            {(kind === "adsb" || kind === "ais") && <TargetsView scope={scope} />}
            {kind === "aprs" && <AprsView scope={scope} />}
            {kind === "pocsag" && <PagerView scope={scope} />}
            {(kind === "rtty" || kind === "morse") && <TextView kind={kind} scope={scope} />}
          </section>
        );
      })}
    </Body>
  );
}

function MapDockPanel() {
  return (
    <Body scroll={false}>
      <MapPanel className="h-full min-h-0 w-full flex-1" />
    </Body>
  );
}

function ScannerDockPanel() {
  const shell = useShell();
  return (
    <Body>
      <ScannerPanel active={shell.active} />
    </Body>
  );
}

function DecoderLogDockPanel() {
  const shell = useShell();
  return (
    <Body scroll={false}>
      <DecoderLogPanel deviceSets={shell.deviceSets} />
    </Body>
  );
}

function PresetsDockPanel() {
  const shell = useShell();
  return (
    <Body>
      <PresetsPanel active={shell.active} />
    </Body>
  );
}

function BookmarksDockPanel() {
  const shell = useShell();
  return (
    <Body>
      <BookmarksPanel active={shell.active} />
    </Body>
  );
}

function TemplatesDockPanel() {
  const shell = useShell();
  return (
    <Body>
      <TemplatesPanel active={shell.active} />
    </Body>
  );
}

function RecordingsDockPanel() {
  const shell = useShell();
  return (
    <Body>
      <RecordingsPanel onSelect={shell.setActiveDs} />
    </Body>
  );
}

/** dockview's component map, keyed by the same string the layout stores. Defined once at module
 * scope: a fresh object per render would remount every panel. */
export const PANEL_COMPONENTS: Record<PanelKind, React.FunctionComponent<IDockviewPanelProps>> = {
  spectrum: SpectrumPanel,
  channels: ChannelsDockPanel,
  decoders: DecodersPanel,
  map: MapDockPanel,
  scanner: ScannerDockPanel,
  decoder_log: DecoderLogDockPanel,
  presets: PresetsDockPanel,
  bookmarks: BookmarksDockPanel,
  templates: TemplatesDockPanel,
  recordings: RecordingsDockPanel,
};

function kindOf(channel: ChannelInfo): string {
  return channel.settings.params.type;
}

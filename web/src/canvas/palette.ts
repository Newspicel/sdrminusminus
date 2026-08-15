// What the "+ Node" menu offers, arranged (: the server describes the palette, the client
// only renders it). The catalog's own `category` does the arranging, so a node kind added
// server-side lands in the right section with no frontend edit.
//
// Channel entries are the exception the catalog asks for: one entry per *descriptor* rather than
// one "Channel" button (`NodeTypeInfo::needs_channel_type`). There are two dozen of them, so they
// are split the way an operator reaches for them — a mode that makes audio, or a decoder that
// makes messages — using the same `has_audio` flag the faces use to decide whether to draw
// volume.
import type { ChannelDescriptor, NodeKind, PatchCatalog, PositionSource } from "../lib/types";

export interface PaletteItem {
  /** Unique across the palette: the React key, and what the filter matches on besides the name. */
  id: string;
  name: string;
  kind: NodeKind;
  /** Set only on channel entries — the type the node is created with. */
  type?: ChannelDescriptor;
  source?: PositionSource;
}

export interface PaletteGroup {
  id: string;
  title: string;
  items: PaletteItem[];
}

/** Section order, and the one place a category's name is written for the operator. `channel` is
 * absent because a channel entry never lands in a section of its own — it is split into the two
 * below it. */
const SECTIONS: readonly { id: string; title: string }[] = [
  { id: "source", title: "Sources" },
  { id: "mode", title: "Modes" },
  { id: "decoder", title: "Decoders" },
  { id: "display", title: "Displays" },
  { id: "sink", title: "Sinks" },
  { id: "feature", title: "Tools" },
];

export function paletteGroups(
  catalog: PatchCatalog,
  channelTypes: readonly ChannelDescriptor[],
  devicePosition = false,
): PaletteGroup[] {
  const sections = new Map<string, PaletteItem[]>(SECTIONS.map((section) => [section.id, []]));
  for (const entry of catalog.nodes) {
    if (entry.kind === "gps") {
      const sources = sections.get("source");
      if (devicePosition) {
        sources?.push({
          id: "gps:device",
          name: "Device GPS",
          kind: "gps",
          source: { type: "device" },
        });
      }
      sources?.push(
        {
          id: "gps:gpsd",
          name: "GPSD",
          kind: "gps",
          source: { type: "gpsd", address: "127.0.0.1:2947" },
        },
        {
          id: "gps:nmea",
          name: "NMEA serial",
          kind: "gps",
          source: {
            type: "nmea",
            device: "/dev/ttyUSB0",
            baud: 9_600,
            update_interval_ms: 1_000,
          },
        },
      );
      continue;
    }
    if (entry.needs_channel_type === true) {
      for (const group of channelGroups(channelTypes)) {
        sections.get(group.id)?.push(...group.items);
      }
      continue;
    }
    sections.get(entry.category)?.push({
      id: entry.kind,
      name: entry.name,
      kind: entry.kind as NodeKind,
    });
  }
  return SECTIONS.map((section) => ({
    ...section,
    items: sections.get(section.id) ?? [],
  })).filter((group) => group.items.length > 0);
}

function channelGroups(channelTypes: readonly ChannelDescriptor[]): PaletteGroup[] {
  const items = new Map<string, PaletteItem[]>([
    ["mode", []],
    ["decoder", []],
  ]);
  for (const type of channelTypes) {
    items.get(type.has_audio ? "mode" : "decoder")?.push({
      id: `channel:${type.type_id}`,
      name: type.name,
      kind: "channel",
      type,
    });
  }
  return SECTIONS.map((section) => ({ ...section, items: items.get(section.id) ?? [] })).filter(
    (group) => group.items.length > 0,
  );
}

/** The channel half of the palette, as a menu offering one at a frequency wants it: what the
 * spectrum suggests under the pointer pinned above the full list, so the ordinary answer is one
 * click and every other mode is still there. */
export function channelPicker(
  channelTypes: readonly ChannelDescriptor[],
  suggested: string,
): PaletteGroup[] {
  const groups = channelGroups(channelTypes);
  const type = channelTypes.find((entry) => entry.type_id === suggested);
  if (type === undefined) {
    return groups;
  }
  return [
    {
      id: "suggested",
      title: "Suggested",
      items: [{ id: `suggested:${type.type_id}`, name: type.name, kind: "channel", type }],
    },
    ...groups,
  ];
}

/** What Enter takes when a search is submitted without a choice: the top item of the first
 * section, which is the suggestion where one is offered. */
export function firstPaletteItem(groups: readonly PaletteGroup[]): PaletteItem | undefined {
  return groups[0]?.items[0];
}

/** The palette narrowed to what matches `query`, sections with nothing left dropped.
 *
 * The type id is matched as well as the name so the terms an operator already knows work —
 * "adsb" finds "ADS-B (1090ES)", which no substring of its name does. */
export function filterPalette(groups: readonly PaletteGroup[], query: string): PaletteGroup[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") {
    return [...groups];
  }
  return groups
    .map((group) => ({
      ...group,
      items: group.items.filter(
        (item) =>
          item.name.toLowerCase().includes(needle) || item.id.toLowerCase().includes(needle),
      ),
    }))
    .filter((group) => group.items.length > 0);
}

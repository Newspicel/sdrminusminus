import type { ChannelDescriptor, NodeKind, PatchCatalog, PositionSource } from "../lib/types";

export interface PaletteItem {
  id: string;
  name: string;
  kind: NodeKind;
  type?: ChannelDescriptor;
  source?: PositionSource;
}

export interface PaletteGroup {
  id: string;
  title: string;
  items: PaletteItem[];
}

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
          id: "gps:fixed",
          name: "Fixed place",
          kind: "gps",
          source: { type: "fixed", lat: 0, lon: 0 },
        },
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

export function firstPaletteItem(groups: readonly PaletteGroup[]): PaletteItem | undefined {
  return groups[0]?.items[0];
}

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

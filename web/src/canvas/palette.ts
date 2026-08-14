// What the "+ Node" menu offers, arranged (: the server describes the palette, the client
// only renders it). The catalog's own `category` does the arranging, so a node kind added
// server-side lands in the right section with no frontend edit.
//
// Channel entries are the exception the catalog asks for: one entry per *descriptor* rather than
// one "Channel" button (`NodeTypeInfo::needs_channel_type`). There are two dozen of them, so they
// are split the way an operator reaches for them — a mode that makes audio, or a decoder that
// makes messages — using the same `has_audio` flag the faces use to decide whether to draw
// volume.
import type { ChannelDescriptor, NodeKind, PatchCatalog } from "../lib/types";

export interface PaletteItem {
  /** Unique across the palette: the React key, and what the filter matches on besides the name. */
  id: string;
  name: string;
  kind: NodeKind;
  /** Set only on channel entries — the type the node is created with. */
  type?: ChannelDescriptor;
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
  { id: "source", title: "Radios" },
  { id: "mode", title: "Modes" },
  { id: "decoder", title: "Decoders" },
  { id: "display", title: "Displays" },
  { id: "sink", title: "Sinks" },
  { id: "feature", title: "Tools" },
];

export function paletteGroups(
  catalog: PatchCatalog,
  channelTypes: readonly ChannelDescriptor[],
): PaletteGroup[] {
  const sections = new Map<string, PaletteItem[]>(SECTIONS.map((section) => [section.id, []]));
  for (const entry of catalog.nodes) {
    if (entry.needs_channel_type === true) {
      for (const type of channelTypes) {
        sections.get(type.has_audio ? "mode" : "decoder")?.push({
          id: `channel:${type.type_id}`,
          name: type.name,
          kind: "channel",
          type,
        });
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

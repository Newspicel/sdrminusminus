import { useState } from "react";
import { FIELD, LABEL } from "../components/controls";
import { formatHz } from "../components/format";
import type { NodeKind, PositionSource } from "../lib/types";
import { useWorkspaceContext } from "./context";
import { filterPalette, type PaletteItem, paletteGroups } from "./palette";

export function NodePalette({
  onAdd,
}: {
  onAdd: (kind: NodeKind, channelType?: string, source?: PositionSource) => void;
}) {
  const workspace = useWorkspaceContext();
  const [query, setQuery] = useState("");
  const groups = filterPalette(
    paletteGroups(
      workspace.context.catalog,
      workspace.context.channelTypes,
      navigator.geolocation !== undefined,
    ),
    query,
  );

  return (
    <div className="flex flex-col gap-2">
      <input
        className={FIELD}
        placeholder="Search nodes — scope, nfm, adsb…"
        aria-label="Search the node palette"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      {groups.length === 0 && (
        <span className="px-1 py-2 text-sm text-ink-dim">Nothing in the palette matches that.</span>
      )}
      {groups.map((group) => (
        <div key={group.id} className="flex flex-col gap-1">
          <span className={`${LABEL} px-1`}>{group.title}</span>
          <div className="grid grid-cols-2 gap-1">
            {group.items.map((item) => (
              <Entry
                key={item.id}
                item={item}
                onAdd={() => onAdd(item.kind, item.type?.type_id, item.source)}
              />
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

function Entry({ item, onAdd }: { item: PaletteItem; onAdd: () => void }) {
  return (
    <button
      type="button"
      className="flex min-w-0 flex-col items-start rounded-[3px] border border-transparent px-2 py-1 text-left transition-colors duration-100 hover:border-accent-dim hover:bg-panel-2"
      onClick={onAdd}
    >
      <span className="w-full truncate text-xs text-ink">{item.name}</span>
      {/* Hidden from the accessible name, which must read as the action alone: the bandwidth is a
          hint about where this will fit, and the face states it again once the node is drawn. */}
      {item.type !== undefined && (
        <span
          aria-hidden
          className="w-full truncate font-mono text-[10px] tabular-nums text-ink-faint"
        >
          {formatHz(item.type.bandwidth_hz)}
        </span>
      )}
    </button>
  );
}

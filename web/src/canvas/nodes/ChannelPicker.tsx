import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";
import { Form, Input } from "../../components/BaseControls";
import { BTN, FIELD, LABEL, SURFACE } from "../../components/controls";
import { formatHz } from "../../components/format";
import type { ChannelDescriptor } from "../../lib/types";
import { PaletteEntry } from "../NodePalette";
import { channelPicker, filterPalette, firstPaletteItem, type PaletteItem } from "../palette";
import type { ScopePick } from "./scopePick";

/**
 * Choosing the mode for a channel drawn at a picked frequency.
 *
 * A dialog rather than a list inside the scope's own menu: a node is not a viewport, so a list
 * there is a scroll box a few rows tall sitting on a surface the canvas pans and zooms with the
 * same wheel. Portalled out of the node, the wheel scrolls the list and nothing else.
 */
export function ChannelPicker({
  pick,
  channelTypes,
  suggested,
  onChannel,
  onClose,
}: {
  pick: ScopePick;
  /** Every mode and decoder a channel can be drawn as, as the server describes them. */
  channelTypes: readonly ChannelDescriptor[];
  /** The mode the picker pins first, already resolved (`channelTypeAt`). */
  suggested: string;
  onChannel: (channelType: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const groups = filterPalette(channelPicker(channelTypes, suggested), query);

  const create = (item: PaletteItem | undefined): void => {
    if (item?.type !== undefined) {
      onChannel(item.type.type_id);
    }
  };

  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-40 bg-bg/70" />
        <Dialog.Popup
          className={`${SURFACE} fixed top-1/2 left-1/2 z-40 flex max-h-[80vh] w-full max-w-lg -translate-x-1/2 -translate-y-1/2 flex-col p-4`}
        >
          <Dialog.Title className="text-base font-medium text-ink">New channel</Dialog.Title>
          <Dialog.Description className="mt-1 font-mono text-xs tabular-nums text-ink-dim">
            {formatHz(pick.hz)}
          </Dialog.Description>

          <Form
            className="mt-3 flex min-h-0 flex-1 flex-col"
            onSubmit={(event) => {
              event.preventDefault();
              create(firstPaletteItem(groups));
            }}
          >
            <Input
              autoFocus
              className={`${FIELD} w-full shrink-0`}
              type="search"
              name="channel-mode-filter"
              aria-label="Search channel modes"
              placeholder="nfm, adsb…"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
            <div className="mt-2 flex min-h-0 flex-col gap-2 overflow-y-auto">
              {groups.length === 0 && (
                <p className="py-3 text-center text-sm text-ink-dim">No mode matches that.</p>
              )}
              {groups.map((group) => (
                <div key={group.id} className="flex flex-col gap-1">
                  <span className={`${LABEL} px-1`}>{group.title}</span>
                  <div className="grid grid-cols-2 gap-1">
                    {group.items.map((item) => (
                      <PaletteEntry key={item.id} item={item} onAdd={() => create(item)} />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </Form>

          <div className="mt-4 flex shrink-0 justify-end">
            <Dialog.Close className={BTN}>Cancel</Dialog.Close>
          </div>
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

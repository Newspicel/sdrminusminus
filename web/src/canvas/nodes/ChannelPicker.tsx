import { Dialog } from "@base-ui/react/dialog";
import { useState } from "react";
import { Form, Input } from "../../components/BaseControls";
import { BTN, FIELD, LABEL, SURFACE } from "../../components/controls";
import { PaletteEntry } from "../NodePalette";
import { filterPalette, firstPaletteItem, type PaletteGroup, type PaletteItem } from "../palette";

export function ChannelPicker({
  title,
  note,
  groups,
  onChannel,
  onClose,
}: {
  title: string;
  note: string;
  groups: readonly PaletteGroup[];
  onChannel: (channelType: string) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const shown = filterPalette(groups, query);

  const choose = (item: PaletteItem | undefined): void => {
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
          <Dialog.Title className="text-base font-medium text-ink">{title}</Dialog.Title>
          <Dialog.Description className="mt-1 font-mono text-xs tabular-nums text-ink-dim">
            {note}
          </Dialog.Description>

          <Form
            className="mt-3 flex min-h-0 flex-1 flex-col"
            onSubmit={(event) => {
              event.preventDefault();
              choose(firstPaletteItem(shown));
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
              {shown.length === 0 && (
                <p className="py-3 text-center text-sm text-ink-dim">No mode matches that.</p>
              )}
              {shown.map((group) => (
                <div key={group.id} className="flex flex-col gap-1">
                  <span className={`${LABEL} px-1`}>{group.title}</span>
                  <div className="grid grid-cols-2 gap-1">
                    {group.items.map((item) => (
                      <PaletteEntry key={item.id} item={item} onAdd={() => choose(item)} />
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

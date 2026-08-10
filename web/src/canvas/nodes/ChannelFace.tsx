// The channel node's face (CANVAS §1): where the channel sits in its receiver's passband, the
// settings its mode owns, and — when the type decodes — the live output that used to be a
// separate decoder panel (CANVAS §8 phase ③). The face is the whole control surface; there is no
// dialog behind it.
import { ChannelControls } from "../../components/ChannelControls";
import {
  channelDecoderKind,
  channelHasAudio,
  exactRateMismatch,
} from "../../components/channelSettings";
import { BTN_PRIMARY } from "../../components/controls";
import { DecoderView } from "../../components/DecoderPanels";
import { formatMhz, formatSignedKhz } from "../../components/format";
import type { PatchNode } from "../../lib/types";
import { sourcesOf, targetsOf } from "../binding";
import { deviceSetOf, useStationContext } from "../context";
import { FaceBody, NodeShell } from "./NodeShell";

export function ChannelFace({ node }: { node: PatchNode }) {
  const station = useStationContext();
  // The node registry mounts this face for channel nodes only; the guard is what narrows `data`.
  if (node.kind !== "channel") {
    return null;
  }

  const typeId = node.data.channel_type;
  const descriptor = station.context.channelTypes.find((type) => type.type_id === typeId);
  const name = descriptor?.name ?? typeId.toUpperCase();
  const set = deviceSetOf(station, node.id);
  const channel = station.channels.get(node.id) ?? null;
  // Wired and bound are different states: a wire to a radio that is unplugged is kept, and the
  // face says which of the two is missing rather than offering a fix that cannot work (CANVAS §3).
  const wired = sourcesOf(station.graph, node.id, "iq").length > 0;
  // Where the channel actually is: the receiver's centre plus the offset, falling back to the
  // offset alone while the radio reports no centre.
  const centerHz = set?.settings.center_hz ?? null;
  const offsetHz = channel?.settings.offset_hz ?? 0;
  const readout = centerHz === null ? formatSignedKhz(offsetHz) : formatMhz(centerHz + offsetHz);
  const needsRateHz = exactRateMismatch(descriptor, set?.settings.sample_rate);
  const decoderKind = channelDecoderKind(descriptor);
  // The face has no play button — audio belongs to the speaker the wire reaches — so a channel
  // that demodulates sound into nothing has to say so somewhere, and this is where it is looked
  // for.
  const audioUnwired =
    channelHasAudio(descriptor) && targetsOf(station.graph, node.id, "audio").length === 0;

  return (
    <NodeShell
      node={node}
      title={name}
      category="channel"
      subtitle={
        channel === null ? (
          bindingLabel(wired, set !== null)
        ) : (
          <span className="font-mono tabular-nums">{readout}</span>
        )
      }
      live={channel !== null}
    >
      <FaceBody>
        {channel === null || set === null ? (
          <Unbound wired={wired} open={set !== null} onApply={station.apply} />
        ) : (
          <>
            {needsRateHz !== null && (
              <p
                role="alert"
                className="border-b border-danger/40 bg-danger/10 px-2 py-1.5 text-xs text-danger"
              >
                {name} needs the receiver at exactly{" "}
                <span className="font-mono tabular-nums">{needsRateHz / 1e6}</span> Msps.
              </p>
            )}
            <ChannelControls
              deviceSet={set.id}
              channel={channel}
              descriptor={descriptor}
              spanHz={set.settings.sample_rate ?? null}
            />
            {audioUnwired && <p className="legend px-2 pb-2">audio out reaches no speaker</p>}
            {decoderKind !== null && (
              <div className="border-t border-line">
                {/* Channel ids are allocated per device set, so two sets both have a channel 1;
                    scoping on the id alone would pour one set's frames into this face. */}
                <DecoderView
                  kind={decoderKind}
                  scope={{ deviceSet: set.id, channel: channel.id }}
                />
              </div>
            )}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

/** The header's one-word account of why there is no channel behind the node. */
function bindingLabel(wired: boolean, open: boolean): string {
  if (!wired) {
    return "no receiver";
  }
  return open ? "not created" : "receiver absent";
}

/** No engine channel behind the node — which of three things is missing. Apply is offered
 * whenever a receiver is wired: it opens an attached radio and creates the channel, and for a
 * radio that is not there it changes nothing, since the patch keeps the wire either way
 * (CANVAS §3). */
function Unbound({ wired, open, onApply }: { wired: boolean; open: boolean; onApply: () => void }) {
  return (
    <div className="flex flex-col items-start gap-2 p-3">
      <p className="text-sm text-ink-dim">
        {!wired
          ? "Nothing feeds this channel — wire a receiver's IQ output into it."
          : open
            ? "The receiver is open, but this channel has not been created on it yet."
            : "Its receiver is not open. Applying opens an attached radio; while none is, the wire is kept and the settings wait."}
      </p>
      {wired && (
        <button type="button" className={BTN_PRIMARY} onClick={onApply}>
          Apply patch
        </button>
      )}
    </div>
  );
}

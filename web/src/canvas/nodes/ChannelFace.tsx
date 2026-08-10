// The channel node's face (CANVAS §1): where the channel sits in its radio's passband, the
// settings its mode owns, and — when the type decodes — the live output that used to be a
// separate decoder panel (CANVAS §8 phase ③). The face is the whole control surface; there is no
// dialog behind it.
import { ChannelControls } from "../../components/ChannelControls";
import {
  channelDecoderKind,
  channelHasAudio,
  rateMismatch,
} from "../../components/channelSettings";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { DecoderView } from "../../components/DecoderPanels";
import { formatMhz, formatSignedKhz } from "../../components/format";
import type { DeviceSet, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
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
  // Where the channel actually is: the radio's centre plus the offset, falling back to the
  // offset alone while the radio reports no centre.
  const centerHz = set?.settings.center_hz ?? null;
  const offsetHz = channel?.settings.offset_hz ?? 0;
  const readout = centerHz === null ? formatSignedKhz(offsetHz) : formatMhz(centerHz + offsetHz);
  const wantedRate = rateMismatch(descriptor, set?.settings.sample_rate);
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
        {wantedRate !== null && set !== null && (
          <RateMismatch name={name} set={set} wanted={wantedRate} />
        )}
        {channel === null || set === null ? (
          <Unbound wired={wired} open={set !== null} onApply={station.apply} />
        ) : (
          <>
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

/**
 * The one refusal an operator meets by accident, so it answers "why" and not just "no", and
 * offers a rate *this* radio actually has: a decoder that reads the device's own samples runs
 * over a range of rates (PLAN §18), and the nearest one inside it is a click away. Naming a
 * number the receiver cannot produce is how "set it to 2.000 MHz" became a dead end on every
 * RTL-SDR, whose nearest rate is 2.048.
 */
function RateMismatch({
  name,
  set,
  wanted,
}: {
  name: string;
  set: DeviceSet;
  wanted: { min: number; max: number };
}) {
  const { applyPatch } = useDevicePatch();
  const offered = nearestRate(set, wanted);
  const range =
    wanted.min === wanted.max
      ? `exactly ${mhz(wanted.min)} MHz`
      : `between ${mhz(wanted.min)} and ${mhz(wanted.max)} MHz`;
  return (
    <div
      role="alert"
      className="flex flex-col items-start gap-1.5 border-b border-danger/40 bg-danger/10 px-2 py-1.5 text-xs text-danger"
    >
      <p>
        {name} reads the radio's own samples, so the radio has to run {range}. At{" "}
        <span className="font-mono tabular-nums">{mhz(set.settings.sample_rate ?? 0)}</span> MHz it
        decodes nothing at all.
      </p>
      {offered === null ? (
        <p>
          This radio offers no rate in that range, so it cannot carry {name}. Another radio has to.
        </p>
      ) : (
        <button
          type="button"
          className={BTN}
          onClick={() => applyPatch(set.id, { sample_rate: offered })}
        >
          Set {set.device.label} to {mhz(offered)} MHz
        </button>
      )}
    </div>
  );
}

/** The rate this radio offers that is closest to the bottom of the range — lowest first, since
 * every extra sample costs the DSP thread and buys the decoder nothing. `null` when the radio
 * offers none. A radio that reports no discrete rates takes any, so it takes the minimum. */
function nearestRate(set: DeviceSet, wanted: { min: number; max: number }): number | null {
  const rates = set.capabilities.sample_rates;
  if (rates.length === 0) {
    return wanted.min;
  }
  const inside = rates.filter((rate) => rate >= wanted.min && rate <= wanted.max);
  return inside.length === 0 ? null : Math.min(...inside);
}

function mhz(hz: number): string {
  return (hz / 1e6).toFixed(3);
}

/** The header's one-word account of why there is no channel behind the node. */
function bindingLabel(wired: boolean, open: boolean): string {
  if (!wired) {
    return "no device";
  }
  return open ? "not created" : "radio absent";
}

/** No engine channel behind the node — which of three things is missing. Apply is offered
 * whenever a radio is wired: it opens an attached radio and creates the channel, and for a
 * radio that is not there it changes nothing, since the patch keeps the wire either way
 * (CANVAS §3). */
function Unbound({ wired, open, onApply }: { wired: boolean; open: boolean; onApply: () => void }) {
  return (
    <div className="flex flex-col items-start gap-2 p-3">
      <p className="text-sm text-ink-dim">
        {!wired
          ? "Nothing feeds this channel — wire a device's IQ output into it."
          : open
            ? "The radio is open, but this channel has not been created on it yet."
            : "Its radio is not open. Applying opens an attached radio; while none is, the wire is kept and the settings wait."}
      </p>
      {wired && (
        <button type="button" className={BTN_PRIMARY} onClick={onApply}>
          Apply patch
        </button>
      )}
    </div>
  );
}

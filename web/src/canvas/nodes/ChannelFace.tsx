import { Button } from "../../components/BaseControls";
import { ChannelControls } from "../../components/ChannelControls";
import { channelHasVideo, rateMismatch } from "../../components/channelSettings";
import { BTN, BTN_PRIMARY } from "../../components/controls";
import { formatMhz, formatSignedKhz } from "../../components/format";
import { LevelMeter } from "../../components/LevelMeter";
import { useLevelStore } from "../../lib/levels";
import type { ChannelDescriptor, DeviceSet, PatchGraph, PatchNode } from "../../lib/types";
import { forStream, useDevicePatch } from "../../lib/useDevicePatch";
import { iqSourceOf, targetsOf } from "../binding";
import { useWorkspaceContext } from "../context";
import { deviceSetOf } from "../workspaceDevice";
import { FaceBody, NodeShell } from "./NodeShell";

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const set = deviceSetOf(workspace, node.id);
  const levels = useLevelStore((state) => (set === null ? undefined : state.byDeviceSet[set.id]));
  if (node.kind !== "channel") {
    return null;
  }

  const typeId = node.data.channel_type;
  const descriptor = workspace.context.channelTypes.find((type) => type.type_id === typeId);
  const name = descriptor?.name ?? typeId.toUpperCase();
  const channel = workspace.channels.get(node.id) ?? null;
  const source = iqSourceOf(workspace.graph, node.id);
  const wired = source !== null;
  // Where the channel actually is: *its lane's* centre plus the offset — on a radio whose
  // streams tune apart, the device-wide centre would file this channel under a frequency it is
  // not on — falling back to the offset alone while the radio reports no centre.
  const centerHz =
    set === null
      ? null
      : (forStream(set.settings, source?.stream ?? 0, set.capabilities.per_stream).center_hz ??
        null);
  const offsetHz = channel?.settings.offset_hz ?? 0;
  const readout = centerHz === null ? formatSignedKhz(offsetHz) : formatMhz(centerHz + offsetHz);
  const wantedRate = rateMismatch(descriptor, set?.settings.sample_rate);
  // The face has no play button and no decoder pane — everything this channel produces is read at
  // the end of a wire — so a channel that demodulates or decodes into nothing has to say so
  // somewhere, and this is where it is looked for.
  const unwired = unwiredOutputs(workspace.graph, node.id, descriptor);

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
          <Unbound wired={wired} open={set !== null} onApply={workspace.apply} />
        ) : (
          <>
            <div className="px-2 pt-2">
              <LevelMeter level={levels?.[channel.id]} squelchDb={channel.settings.squelch_db} />
            </div>
            <ChannelControls
              deviceSet={set.id}
              channel={channel}
              descriptor={descriptor}
              spanHz={set.settings.sample_rate ?? null}
            />
            {unwired.map((reason) => (
              <p key={reason} className="legend px-2 pb-2">
                {reason}
              </p>
            ))}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

/**
 * The outputs this channel's type has that no face is reading, phrased as what is missing at the
 * far end. A stream that arrives nowhere looks exactly like one that never started, and this is
 * the only place the difference shows.
 *
 * Only video. `events` is absent because every NFM channel declares the `tone` decoder and almost
 * none is set to look for one; `audio` because an unwired speaker is the normal state of every
 * decoder-only channel, so the line fired on nearly every face in the patch.
 */
function unwiredOutputs(
  graph: PatchGraph,
  node: string,
  descriptor: ChannelDescriptor | undefined,
): string[] {
  const reaches = (port: string): boolean => targetsOf(graph, node, port).length > 0;
  return channelHasVideo(descriptor) && !reaches("video") ? ["video out reaches no screen"] : [];
}

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
        <Button
          type="button"
          className={BTN}
          onClick={() => applyPatch(set.id, { sample_rate: offered })}
        >
          Set {set.device.label} to {mhz(offered)} MHz
        </Button>
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
        <Button type="button" className={BTN_PRIMARY} onClick={onApply}>
          Apply patch
        </Button>
      )}
    </div>
  );
}

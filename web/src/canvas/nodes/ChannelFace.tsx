import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { ChannelControls } from "../../components/ChannelControls";
import { channelHasAudio, channelHasVideo, rateMismatch } from "../../components/channelSettings";
import { formatMhz, formatSignedKhz } from "../../components/format";
import type { ChannelDescriptor, DeviceSet, PatchGraph, PatchNode } from "../../lib/types";
import { forStream, useDevicePatch } from "../../lib/useDevicePatch";
import { iqSourceOf, targetsOf } from "../binding";
import { deviceSetOf, useWorkspaceContext } from "../context";
import { FaceBody, NodeShell } from "./NodeShell";

export function ChannelFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "channel") {
    return null;
  }

  const typeId = node.data.channel_type;
  const descriptor = workspace.context.channelTypes.find((type) => type.type_id === typeId);
  const name = descriptor?.name ?? typeId.toUpperCase();
  const set = deviceSetOf(workspace, node.id);
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
            <ChannelControls
              deviceSet={set.id}
              channel={channel}
              descriptor={descriptor}
              spanHz={set.settings.sample_rate ?? null}
            />
            {unwired.map((reason) => (
              <p
                key={reason}
                className="font-mono text-[10px] leading-[1.4] tracking-[0.09em] uppercase text-muted-foreground/70 px-2 pb-2"
              >
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
 * The `events` port is deliberately absent: every NFM channel declares the `tone` decoder and
 * almost none of them is set to look for one, so a line here would fire on the commonest channel
 * in the app to report something that is usually not a mistake.
 */
function unwiredOutputs(
  graph: PatchGraph,
  node: string,
  descriptor: ChannelDescriptor | undefined,
): string[] {
  const reaches = (port: string): boolean => targetsOf(graph, node, port).length > 0;
  const missing: string[] = [];
  if (channelHasAudio(descriptor) && !reaches("audio")) {
    missing.push("audio out reaches no speaker");
  }
  if (channelHasVideo(descriptor) && !reaches("video")) {
    missing.push("video out reaches no screen");
  }
  return missing;
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
    <Alert variant="destructive" className="rounded-none border-x-0 border-t-0 text-xs">
      <AlertDescription className="space-y-1.5">
        <p>
          {name} reads the radio's own samples, so the radio has to run {range}. At{" "}
          <span className="font-mono tabular-nums">{mhz(set.settings.sample_rate ?? 0)}</span> MHz
          it decodes nothing at all.
        </p>
        {offered === null ? (
          <p>
            This radio offers no rate in that range, so it cannot carry {name}. Another radio has
            to.
          </p>
        ) : (
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => applyPatch(set.id, { sample_rate: offered })}
          >
            Set {set.device.label} to {mhz(offered)} MHz
          </Button>
        )}
      </AlertDescription>
    </Alert>
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
      <p className="text-sm text-muted-foreground">
        {!wired
          ? "Nothing feeds this channel — wire a device's IQ output into it."
          : open
            ? "The radio is open, but this channel has not been created on it yet."
            : "Its radio is not open. Applying opens an attached radio; while none is, the wire is kept and the settings wait."}
      </p>
      {wired && (
        <Button type="button" size="sm" onClick={onApply}>
          Apply patch
        </Button>
      )}
    </div>
  );
}

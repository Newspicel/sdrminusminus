import { Chips, ChoiceChip, ToggleChip } from "../../components/face/Chips";
import { FaceFault } from "../../components/face/Fault";
import { Readout, Readouts } from "../../components/face/Readouts";
import { TextChip } from "../../components/face/TextChip";
import { formatHz } from "../../components/format";
import type { PatchNode, TrunkSystemStatus } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { ChannelPlanTable } from "./ChannelPlanTable";
import {
  adoptable,
  awaitingControlChannel,
  channelPlanRows,
  controlChannelStalled,
  DMR_TRUNK_PROTOCOLS,
  formatSearchRanges,
  otherControlLabel,
  parseControlHz,
  parseSearchRanges,
  planLabel,
  searchSummary,
  trunkProtocolLabel,
} from "./dmrTrunk";
import { FaceBody, NodeShell } from "./NodeShell";

export function DmrTrunkFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "dmr_trunk") {
    return null;
  }
  const status = workspace.trunks.find((system) => system.node === node.id);
  const onIq = (workspace.graph.edges ?? []).some(
    (edge) => edge.to.node === node.id && edge.to.port === "iq",
  );
  const awaiting = awaitingControlChannel(onIq, node.data.control_hz);
  const stalled = controlChannelStalled(onIq, node.data.control_hz, status?.carriers);
  const protocol = node.data.protocol ?? "auto";
  const detected = status?.detected ?? null;
  const discovery = node.data.discovery ?? { enabled: false, ranges: [], max_probes: 0 };
  const channelMap = node.data.channel_map ?? [];
  const learned = status?.channel_map ?? [];
  const probes = status?.probes ?? [];
  const followers = status?.followers ?? [];
  const summary = searchSummary(
    discovery.ranges,
    status?.candidates ?? 0,
    status?.searching ?? 0,
    probes.length,
  );
  const following = new Set(
    followers
      .map((follower) => follower.logical_channel)
      .filter((lcn): lcn is number => lcn != null),
  );
  const edit = (next: Partial<typeof node.data>) => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "dmr_trunk" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));
  };
  return (
    <NodeShell
      node={node}
      title="DMR trunk system"
      category="tool"
      subtitle={
        awaiting ? (
          <span className="text-warn">no control channel</span>
        ) : onIq ? (
          [
            protocol === "auto" ? trunkProtocolLabel(protocol, detected) : null,
            `${followers.length} following`,
          ]
            .filter(Boolean)
            .join(" · ")
        ) : undefined
      }
    >
      <FaceBody>
        <Chips className="p-2">
          <ChoiceChip
            label="Protocol"
            title="Trunking protocol"
            value={protocol}
            options={DMR_TRUNK_PROTOCOLS}
            quiet={protocol === "auto"}
            onChange={(next) => edit({ protocol: next })}
          />
          <TextChip
            label="Control"
            name="Control channel"
            title={
              awaiting
                ? "Name the control channel in MHz. The radio stays untuned until then"
                : "Control channel in MHz"
            }
            value={node.data.control_hz == null ? "" : (node.data.control_hz / 1e6).toString()}
            shown={node.data.control_hz == null ? "none" : undefined}
            placeholder="451.0125"
            onCommit={(text) => edit({ control_hz: parseControlHz(text) })}
          />
          <ToggleChip
            label="Search"
            title="Find the rest of the site's channels"
            on={discovery.enabled ?? false}
            onChange={(next) => edit({ discovery: { ...discovery, enabled: next } })}
          />
          {discovery.enabled === true && (
            <TextChip
              label="Range"
              name="Search range"
              title={`Narrow the search to start-end in MHz / step in kHz. ${summary}`}
              value={formatSearchRanges(discovery.ranges)}
              placeholder="whole band"
              onCommit={(text) =>
                edit({ discovery: { ...discovery, ranges: parseSearchRanges(text) } })
              }
            />
          )}
        </Chips>
        <TrunkActivity status={status} />
        {stalled && (
          <FaceFault
            message="Control channel not running"
            detail="Check it sits inside the radio's passband."
          />
        )}
        {status?.problems.map((problem) => (
          <FaceFault
            key={`${problem.freq_hz}-${problem.slot}`}
            message={`Cannot follow ${formatHz(problem.freq_hz)} TS ${problem.slot}`}
            detail={problem.reason}
          />
        ))}
        <ChannelPlanTable
          label={planLabel(protocol, detected)}
          rows={channelPlanRows(learned, channelMap)}
          entries={channelMap}
          found={adoptable(learned, channelMap)}
          following={following}
          onChange={(channel_map) => edit({ channel_map })}
        />
      </FaceBody>
    </NodeShell>
  );
}

function TrunkActivity({ status }: { status: TrunkSystemStatus | undefined }) {
  const followers = status?.followers ?? [];
  const probes = status?.probes ?? [];
  const otherControl = status?.other_control_hz ?? [];
  if (followers.length === 0 && probes.length === 0 && otherControl.length === 0) {
    return null;
  }
  return (
    <Readouts>
      {followers.map((follower) => (
        <Readout key={`${follower.freq_hz}-${follower.slot}`} label={`TS ${follower.slot}`}>
          {formatHz(follower.freq_hz)}
          {follower.logical_channel == null ? "" : ` · LCN ${follower.logical_channel}`}
        </Readout>
      ))}
      {probes.length > 0 && (
        <Readout label="Listening" title="Search receivers">
          {probes.map((probe) => formatHz(probe.freq_hz)).join(", ")}
        </Readout>
      )}
      {otherControl.length > 0 && (
        <Readout
          label="Also control"
          title="The site runs a control channel here too. Point the node at it if this one stops"
        >
          {otherControlLabel(otherControl)}
        </Readout>
      )}
    </Readouts>
  );
}

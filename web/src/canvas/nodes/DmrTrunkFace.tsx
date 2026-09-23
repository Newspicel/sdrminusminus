import { Input } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { CHIP, FIELD } from "../../components/controls";
import { formatHz } from "../../components/format";
import { Select } from "../../components/Select";
import { SettingGroup, SettingRow, Settings } from "../../components/Settings";
import { FieldUnitFrame, unitPadding } from "../../components/Unit";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { ChannelPlanTable } from "./ChannelPlanTable";
import {
  adoptable,
  awaitingControlChannel,
  channelPlanRows,
  controlChannelLabel,
  controlChannelStalled,
  DMR_TRUNK_PROTOCOLS,
  formatSearchRanges,
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
  const otherControl = status?.other_control_hz ?? [];
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
        onIq && !awaiting
          ? [
              protocol === "auto" ? trunkProtocolLabel(protocol, detected) : null,
              `${followers.length} following`,
            ]
              .filter(Boolean)
              .join(" · ")
          : undefined
      }
    >
      <FaceBody>
        <Settings className="border-b border-line p-2">
          <SettingRow label="Protocol">
            <Select
              label="Protocol"
              value={protocol}
              options={DMR_TRUNK_PROTOCOLS}
              onChange={(next) => edit({ protocol: next })}
            />
          </SettingRow>
          <SettingRow label="Control">
            <FieldUnitFrame symbol="MHz" className="w-32">
              <Input
                aria-label="Control channel"
                className={`${FIELD} w-full`}
                style={unitPadding("MHz")}
                defaultValue={
                  node.data.control_hz == null ? "" : (node.data.control_hz / 1e6).toString()
                }
                placeholder="451.0125"
                onBlur={(event) => edit({ control_hz: parseControlHz(event.target.value) })}
              />
            </FieldUnitFrame>
          </SettingRow>
          <SettingRow label="Record calls">
            <Checkbox
              label="Record calls"
              checked={node.data.record_calls ?? true}
              onChange={(next) => edit({ record_calls: next })}
            />
          </SettingRow>
        </Settings>
        {awaiting && (
          <p role="alert" className="border-b border-line p-2 text-xs text-warning">
            The radio stays untuned until you name the control channel.
          </p>
        )}
        {stalled && (
          <p role="alert" className="border-b border-line p-2 text-xs text-warning">
            The control channel is not running. Check it sits inside the radio's passband.
          </p>
        )}
        <ChannelPlanTable
          label={planLabel(protocol, detected)}
          rows={channelPlanRows(learned, channelMap)}
          entries={channelMap}
          found={adoptable(learned, channelMap)}
          following={following}
          onChange={(channel_map) => edit({ channel_map })}
        />
        <Settings className="border-b border-line p-2">
          <SettingGroup label="Find the rest">
            <SettingRow label="Search">
              <Checkbox
                label="Search"
                checked={discovery.enabled ?? false}
                onChange={(next) => edit({ discovery: { ...discovery, enabled: next } })}
              />
            </SettingRow>
            {discovery.enabled === true && (
              <>
                <SettingRow
                  label="Range"
                  title="Optional: narrow the search to start-end in MHz / step in kHz"
                >
                  <Input
                    aria-label="Search range"
                    className={FIELD}
                    defaultValue={formatSearchRanges(discovery.ranges)}
                    placeholder="whole band"
                    onBlur={(event) =>
                      edit({
                        discovery: {
                          ...discovery,
                          ranges: parseSearchRanges(event.target.value),
                        },
                      })
                    }
                  />
                </SettingRow>
                {summary !== "" && <p className="col-span-2 text-xs text-ink-dim">{summary}</p>}
              </>
            )}
          </SettingGroup>
        </Settings>
        {otherControl.length > 0 && (
          <ul className="flex flex-wrap gap-1 border-b border-line p-2">
            {otherControl.map((freq_hz) => (
              <li
                key={freq_hz}
                className={`${CHIP} text-ink-dim`}
                title="The site runs a control channel here too. Point the node at it if this one stops."
              >
                {controlChannelLabel(freq_hz)}
              </li>
            ))}
          </ul>
        )}
        {probes.length > 0 && (
          <ul className="flex flex-wrap gap-1 border-b border-line p-2">
            {probes.map((probe) => (
              <li key={probe.freq_hz} className={`${CHIP} text-ink-dim`}>
                listening {formatHz(probe.freq_hz)}
              </li>
            ))}
          </ul>
        )}
        {followers.length > 0 && (
          <ul className="flex flex-wrap gap-1 border-b border-line p-2">
            {followers.map((follower) => (
              <li key={`${follower.freq_hz}-${follower.slot}`} className={CHIP}>
                {formatHz(follower.freq_hz)} TS {follower.slot}
                {follower.logical_channel == null ? "" : ` · LCN ${follower.logical_channel}`}
              </li>
            ))}
          </ul>
        )}
        {status?.problems.map((problem) => (
          <p
            key={`${problem.freq_hz}-${problem.slot}`}
            role="alert"
            className="border-b border-line p-2 text-xs text-warning"
          >
            Cannot follow {formatHz(problem.freq_hz)} TS {problem.slot}: {problem.reason}
          </p>
        ))}
      </FaceBody>
    </NodeShell>
  );
}

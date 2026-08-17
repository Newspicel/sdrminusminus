import { Input } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { CHIP, FIELD } from "../../components/controls";
import { Select } from "../../components/Select";
import { SettingGroup, SettingRow, Settings } from "../../components/Settings";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { ChannelPlanTable } from "./ChannelPlanTable";
import {
  adoptable,
  channelPlanRows,
  DMR_TRUNK_PROTOCOLS,
  dmrTrunkGuidance,
  followsTierThree,
  formatChannelMap,
  formatSearchRanges,
  parseChannelMap,
  parseControlHz,
  parseSearchRanges,
  searchSummary,
} from "./dmrTrunk";
import { FaceBody, FaceEmpty, NodeShell } from "./NodeShell";

export function DmrTrunkFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  if (node.kind !== "dmr_trunk") {
    return null;
  }
  const status = workspace.trunks.find((system) => system.node === node.id);
  const sources = (workspace.graph.edges ?? [])
    .filter((edge) => edge.to.node === node.id && edge.to.port === "events")
    .map((edge) => edge.from.node);
  const onIq = (workspace.graph.edges ?? []).some(
    (edge) => edge.to.node === node.id && edge.to.port === "iq",
  );
  const protocol = node.data.protocol ?? "auto";
  const discovery = node.data.discovery ?? { enabled: false, ranges: [], max_probes: 0 };
  const channelMap = node.data.channel_map ?? [];
  const learned = status?.channel_map ?? [];
  const probes = status?.probes ?? [];
  const found = adoptable(learned, channelMap);
  const rows = channelPlanRows(learned, channelMap);
  const following = new Set(
    (status?.followers ?? [])
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
      category="feature"
      subtitle={
        sources.length > 0 || onIq
          ? `${status?.carriers ?? 0} carrier${status?.carriers === 1 ? "" : "s"} · ${status?.followers.length ?? 0} following`
          : undefined
      }
      live={sources.length > 0 || onIq}
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
          <SettingRow label="Record calls">
            <Checkbox
              label="Record calls"
              checked={node.data.record_calls ?? true}
              onChange={(next) => edit({ record_calls: next })}
            />
          </SettingRow>
          {onIq && (
            <SettingRow label="Control" title="Where the control channel sits, in MHz">
              <Input
                aria-label="Control channel"
                className={FIELD}
                defaultValue={
                  node.data.control_hz == null ? "" : (node.data.control_hz / 1e6).toString()
                }
                placeholder="451.0125"
                onBlur={(event) => edit({ control_hz: parseControlHz(event.target.value) })}
              />
            </SettingRow>
          )}
        </Settings>
        {followsTierThree(protocol, status?.detected ?? null) && (
          <Settings className="border-b border-line p-2">
            <SettingGroup label="Channel plan">
              <SettingRow
                label="Known"
                title="Logical channel number and its downlink frequency in MHz"
              >
                <Input
                  aria-label="Known channels"
                  className={FIELD}
                  defaultValue={formatChannelMap(channelMap)}
                  placeholder="17 = 451.0125; 18 = 451.025"
                  onBlur={(event) => edit({ channel_map: parseChannelMap(event.target.value) })}
                />
              </SettingRow>
            </SettingGroup>
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
                  <SettingRow label="Range" title="Start-end in MHz / step in kHz">
                    <Input
                      aria-label="Search range"
                      className={FIELD}
                      defaultValue={formatSearchRanges(discovery.ranges)}
                      placeholder="451.0-451.5 / 12.5"
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
                  <p className="col-span-2 text-xs text-ink-dim">
                    {searchSummary(discovery.ranges, status?.searching ?? 0, probes.length)} A grant
                    names a logical channel, never a frequency, so the search listens across the
                    range and keeps the frequency whose traffic answers the grant.
                  </p>
                </>
              )}
            </SettingGroup>
          </Settings>
        )}
        {sources.length === 0 && !onIq ? (
          <FaceEmpty>
            Wire a radio into the IQ input and name the control channel, or wire DMR decoder events
            in.
          </FaceEmpty>
        ) : (
          <>
            <p className="border-b border-line p-2 text-xs text-ink-dim">
              {dmrTrunkGuidance(protocol, status?.detected ?? null)} Runs on the server while this
              page is closed.
            </p>
            <ChannelPlanTable
              rows={rows}
              entries={channelMap}
              found={found}
              following={following}
              onChange={(channel_map) => edit({ channel_map })}
            />
            {probes.length > 0 && (
              <ul className="flex flex-wrap gap-1 border-b border-line p-2">
                {probes.map((probe) => (
                  <li key={probe.freq_hz} className={`${CHIP} text-ink-dim`}>
                    listening {(probe.freq_hz / 1e6).toFixed(4)} MHz
                  </li>
                ))}
              </ul>
            )}
            {status !== undefined && status.followers.length > 0 && (
              <ul className="flex flex-wrap gap-1 border-b border-line p-2">
                {status.followers.map((follower) => (
                  <li key={`${follower.freq_hz}-${follower.slot}`} className={CHIP}>
                    {(follower.freq_hz / 1e6).toFixed(4)} MHz TS {follower.slot}
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
                Cannot follow {(problem.freq_hz / 1e6).toFixed(4)} MHz TS {problem.slot}:{" "}
                {problem.reason}
              </p>
            ))}
          </>
        )}
      </FaceBody>
    </NodeShell>
  );
}

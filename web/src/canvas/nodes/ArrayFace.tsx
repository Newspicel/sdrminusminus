import { Checkbox } from "../../components/Checkbox";
import type { Options } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { tuningRange } from "../../components/dial";
import { FrequencyDial } from "../../components/FrequencyDial";
import { RadioSettings } from "../../components/RadioSettings";
import { Select } from "../../components/Select";
import { SettingRow, Settings } from "../../components/Settings";
import type { ArrayNode, Coherence, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import { arrayKey, arrayMembers } from "./arrayNode";
import { deviceDialId, refLabel, tuneDelta, tunerDials } from "./deviceNode";
import { FaceBody, FaceEmpty, NodeShell, useFaceActive } from "./NodeShell";

const TIERS: Options<Coherence> = [
  { value: "time_sync", label: "Shared clock" },
  { value: "phase_coherent", label: "Shared clock and LO" },
];

export function ArrayFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const { applyPatch } = useDevicePatch();
  const active = useFaceActive();
  if (node.kind !== "array") {
    return null;
  }
  const settings = node.data;
  const update = (next: Partial<ArrayNode>): void => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: patchNode(snapshot.graph, node.id, (current) =>
        current.kind === "array" ? { ...current, data: { ...current.data, ...next } } : current,
      ),
    }));
  };
  const members = arrayMembers(workspace.graph, node.id);
  const set = workspace.deviceSets.find(
    (candidate) => deviceId(candidate.device) === `array:${arrayKey(node.id)}`,
  );

  return (
    <NodeShell
      node={node}
      title={node.label ?? "Array"}
      category="source"
      subtitle={
        set === undefined && members.length < 2
          ? `${members.length} of 2 radios`
          : `${members.length} elements`
      }
      live={set !== undefined}
    >
      <FaceBody>
        {members.length === 0 && (
          <FaceEmpty>Wire a radio into an input. Each one becomes an element.</FaceEmpty>
        )}
        {set !== undefined && (
          <div className="flex flex-col gap-1 border-line border-b p-2">
            {tunerDials(set).map((dial) => (
              <FrequencyDial
                key={dial.stream}
                id={deviceDialId(node.id, dial.stream)}
                hz={dial.hz}
                range={tuningRange(set.capabilities)}
                wheelTunes={active}
                onTune={(hz) => applyPatch(set.id, tuneDelta(set.capabilities, dial.stream, hz))}
              />
            ))}
          </div>
        )}
        <Settings className="p-2">
          {members.map((member, index) => (
            <SettingRow key={member.node} label={`Element ${index + 1}`}>
              <span className="min-w-0 flex-1 truncate text-sm" title={member.node}>
                {member.device === null ? "no radio picked" : refLabel(member.device)}
              </span>
            </SettingRow>
          ))}
          <SettingRow label="Wired as">
            <Select
              label="What the radios share"
              value={settings.coherence === "none" ? "time_sync" : settings.coherence}
              onChange={(coherence) => update({ coherence })}
              options={TIERS}
            />
          </SettingRow>
          <SettingRow label="Tuned together">
            <Checkbox
              label="Every element follows one centre frequency"
              checked={settings.shared_tuning}
              onChange={(shared_tuning) => update({ shared_tuning })}
            />
          </SettingRow>
        </Settings>
        {set !== undefined && <RadioSettings active={set} className="border-line border-t p-2" />}
      </FaceBody>
    </NodeShell>
  );
}

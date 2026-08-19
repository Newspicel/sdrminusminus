import { useQuery } from "@tanstack/react-query";
import { Button } from "../../components/BaseControls";
import { Checkbox } from "../../components/Checkbox";
import { BTN_QUIET, type Options } from "../../components/controls";
import { deviceId } from "../../components/devices";
import { tuningRange } from "../../components/dial";
import { FrequencyDial } from "../../components/FrequencyDial";
import { RadioSettings } from "../../components/RadioSettings";
import { Select } from "../../components/Select";
import { SettingNote, SettingRow, Settings } from "../../components/Settings";
import { devicesQuery } from "../../lib/api";
import type { ArrayNode, Coherence, DeviceInfo, PatchNode } from "../../lib/types";
import { useDevicePatch } from "../../lib/useDevicePatch";
import { useWorkspaceContext } from "../context";
import { patchNode } from "../graph";
import {
  arrayMembers,
  MAX_ARRAY_MEMBERS,
  moveMember,
  withMember,
  withoutMember,
} from "./arrayNode";
import { deviceDialId, tuneDelta, tunerDials } from "./deviceNode";
import { FaceBody, NodeShell, useFaceActive } from "./NodeShell";

const TIERS: Options<Coherence> = [
  { value: "time_sync", label: "Shared clock" },
  { value: "phase_coherent", label: "Shared clock and LO" },
];

const TIER_NOTE: Record<Coherence, string> = {
  none: "",
  time_sync:
    "Delay between lanes is meaningful, so passive radar works. Every retune scrambles the phase between radios, so bearings need a pilot the calibration can re-solve against.",
  phase_coherent:
    "The radios share a synthesizer as well as a clock, so phase between lanes survives a retune and bearings are meaningful.",
};

export function ArrayFace({ node }: { node: PatchNode }) {
  const workspace = useWorkspaceContext();
  const devices = useQuery(devicesQuery());
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
  const attached = devices.data?.devices ?? [];
  const members = arrayMembers(settings, attached);
  const free = attached.filter(
    (device) => !settings.members.includes(deviceId(device)) && device.driver !== "array",
  );
  const set = workspace.deviceSets.find((candidate) => candidate.device.key === arrayKey(node.id));

  return (
    <NodeShell
      node={node}
      title={node.label ?? "Array"}
      category="source"
      subtitle={`${settings.members.length} radios`}
      live={set !== undefined}
    >
      <FaceBody>
        {set !== undefined && (
          <div className="flex flex-col gap-1 border-b border-line p-2">
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
            <SettingRow key={member.id} label={`Lane ${index + 1}`}>
              <span className="min-w-0 flex-1 truncate text-sm" title={member.id}>
                {member.label}
              </span>
              <Button
                type="button"
                className={BTN_QUIET}
                title="Move this radio one lane earlier"
                disabled={index === 0}
                onClick={() => update({ members: moveMember(settings.members, index, -1) })}
              >
                ↑
              </Button>
              <Button
                type="button"
                className={BTN_QUIET}
                title="Take this radio out of the array"
                onClick={() => update({ members: withoutMember(settings.members, member.id) })}
              >
                Remove
              </Button>
            </SettingRow>
          ))}
          {settings.members.length < MAX_ARRAY_MEMBERS && (
            <SettingRow label="Add radio">
              <Select
                label="A radio to add to the array"
                value=""
                onChange={(id) => {
                  if (id !== "") {
                    update({ members: withMember(settings.members, id) });
                  }
                }}
                options={[
                  { value: "", label: free.length === 0 ? "nothing free" : "pick a radio" },
                  ...free.map((device: DeviceInfo) => ({
                    value: deviceId(device),
                    label: device.label,
                  })),
                ]}
              />
            </SettingRow>
          )}
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
              label="Every member follows one centre frequency"
              checked={settings.shared_tuning}
              onChange={(shared_tuning) => update({ shared_tuning })}
            />
          </SettingRow>
          <SettingNote>{TIER_NOTE[settings.coherence]}</SettingNote>
          {settings.members.length < 2 && (
            <SettingNote>
              An array needs two radios or more. Below that nothing is opened, and the members stay
              free for a device node.
            </SettingNote>
          )}
        </Settings>
        {set !== undefined && <RadioSettings active={set} className="border-t border-line p-2" />}
      </FaceBody>
    </NodeShell>
  );
}

function arrayKey(nodeId: string): string {
  return nodeId.replaceAll(":", "-");
}

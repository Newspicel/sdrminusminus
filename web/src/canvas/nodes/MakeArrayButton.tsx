import { Button } from "../../components/BaseControls";
import { BTN_QUIET } from "../../components/controls";
import { pushToast } from "../../lib/toasts";
import type { DeviceSet, PatchGraph } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { newNodeId, nodeIds, rxStreamCount } from "../graph";
import { canMakeArray, MAKE_ARRAY_TITLE, makeArray } from "../makeArray";
import { defaultBody } from "../newNode";
import { heldLanes } from "./arrayNode";

export function offersMakeArray(graph: PatchGraph, node: string, set: DeviceSet): boolean {
  return rxStreamCount(set.capabilities) >= 2 && heldLanes(graph, node).size === 0;
}

export function MakeArrayButton({ node, set }: { node: string; set: DeviceSet }) {
  const workspace = useWorkspaceContext();
  const check = canMakeArray(workspace.graph, node, set);
  const make = (): void => {
    const body = defaultBody(workspace.context.catalog, "array");
    if (body?.kind !== "array") {
      pushToast("Unknown node: array");
      return;
    }
    if (!check.ok) {
      return;
    }
    const id = newNodeId("array", nodeIds(workspace.graph));
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: makeArray(snapshot.graph, node, check.lanes, body, id),
    }));
    workspace.select(id);
    workspace.apply();
  };
  const button = (
    <Button
      type="button"
      className={BTN_QUIET}
      title={check.ok ? MAKE_ARRAY_TITLE : check.reason}
      disabled={!check.ok}
      onClick={make}
    >
      Make array
    </Button>
  );
  return check.ok ? button : <span title={check.reason}>{button}</span>;
}

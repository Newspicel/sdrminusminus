import { useEffect } from "react";
import { useWorkspaceContext } from "./context";
import { FACES, faceSize } from "./nodes";
import { isTyping } from "./useHotkeys";

export function FullFace() {
  const workspace = useWorkspaceContext();
  const expanded = workspace.expanded;
  const expand = workspace.expand;
  const node = workspace.graph.nodes.find((candidate) => candidate.id === expanded) ?? null;
  const missing = expanded !== null && node === null;

  useEffect(() => {
    if (missing) {
      expand(null);
    }
  }, [missing, expand]);

  useEffect(() => {
    if (node === null) {
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || isTyping(event.target) || inPopup(event.target)) {
        return;
      }
      event.stopPropagation();
      expand(null);
    };
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, [node, expand]);

  if (node === null) {
    return null;
  }
  const Face = FACES[node.kind];
  return (
    <div
      data-full={node.id}
      className="absolute inset-0 z-30 flex items-center justify-center bg-bg p-px"
    >
      <div className="max-h-full max-w-full" style={faceSize(node)}>
        <Face node={node} />
      </div>
    </div>
  );
}

function inPopup(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && target.closest('[role="dialog"], [role="menu"]') !== null;
}

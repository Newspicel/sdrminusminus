import { useRef, useState } from "react";
import { Button, Input } from "../../components/BaseControls";
import type { PatchNode } from "../../lib/types";
import { useWorkspaceContext } from "../context";
import { MAX_NAME_LEN, patchNode } from "../graph";
import { nodeLabel } from "../nodeName";

const TEXT = "min-w-0 truncate text-[12.5px] font-semibold tracking-[0.01em] text-ink";

export function NodeName({ node, title }: { node: PatchNode; title: string }) {
  const workspace = useWorkspaceContext();
  const [draft, setDraft] = useState<string | null>(null);
  const kept = useRef(true);
  const name = node.label ?? title;
  const start = () => {
    kept.current = true;
    setDraft(name);
  };

  if (draft === null) {
    return (
      <Button
        type="button"
        className={`${TEXT} cursor-text text-left`}
        aria-label={`Rename ${name}`}
        title="Double-click to rename"
        onDoubleClick={start}
        onKeyDown={(event) => {
          if (event.key === "F2" || event.key === "Enter") {
            event.preventDefault();
            start();
          }
        }}
      >
        {name}
      </Button>
    );
  }

  const finish = () => {
    setDraft(null);
    const label = nodeLabel(draft, title);
    if (kept.current && label !== (node.label ?? undefined)) {
      workspace.edit((snapshot) => ({
        ...snapshot,
        graph: patchNode(snapshot.graph, node.id, (current) => ({ ...current, label })),
      }));
    }
  };

  return (
    <Input
      autoFocus
      className={`${TEXT} nodrag nopan w-36 rounded-[2px] bg-panel px-1 outline outline-accent`}
      aria-label="Node name"
      maxLength={MAX_NAME_LEN}
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onFocus={(event) => event.currentTarget.select()}
      onBlur={finish}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.nativeEvent.isComposing) {
          return;
        }
        if (event.key === "Enter" || event.key === "Escape") {
          kept.current = event.key === "Enter";
          event.currentTarget.blur();
        }
      }}
    />
  );
}

import { useState } from "react";
import { Button, Form, Input } from "../components/BaseControls";
import { BTN_PRIMARY, FIELD } from "../components/controls";
import { MAX_NAME_LEN } from "./graph";
import { DEFAULT_WORKSPACE_NAME, workspaceName } from "./workspaceName";

export function WorkspaceStart({ onCreate }: { onCreate: (name: string) => void }) {
  const [typed, setTyped] = useState("");

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center">
      <Form
        className="flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          onCreate(workspaceName(typed));
          setTyped("");
        }}
      >
        <Input
          autoFocus
          className={`${FIELD} w-56`}
          aria-label="Name for the new workspace"
          placeholder={DEFAULT_WORKSPACE_NAME}
          maxLength={MAX_NAME_LEN}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
        />
        <Button type="submit" className={BTN_PRIMARY}>
          Create a workspace
        </Button>
      </Form>
    </div>
  );
}

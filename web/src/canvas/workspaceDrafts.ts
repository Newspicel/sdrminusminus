import type { WorkspaceSnapshot } from "../lib/types";

export interface WorkspaceDraft {
  snapshot: WorkspaceSnapshot;
  revision: number;
  generation: number;
}

/** Local workspace state that must outlive query-cache refetches while writes are queued. */
export class WorkspaceDrafts {
  readonly #drafts = new Map<number, WorkspaceDraft>();

  get(id: number): WorkspaceDraft | undefined {
    return this.#drafts.get(id);
  }

  stage(id: number, snapshot: WorkspaceSnapshot, revision: number): WorkspaceDraft {
    const previous = this.#drafts.get(id);
    const draft = {
      snapshot,
      revision: previous?.revision ?? revision,
      generation: (previous?.generation ?? 0) + 1,
    };
    this.#drafts.set(id, draft);
    return draft;
  }

  accepted(id: number, revision: number): WorkspaceSnapshot | undefined {
    const draft = this.#drafts.get(id);
    if (draft === undefined) {
      return undefined;
    }
    this.#drafts.set(id, { ...draft, revision });
    return draft.snapshot;
  }

  /** Drop only the last write for this workspace; a newer generation remains the local draft. */
  finish(id: number, generation: number): boolean {
    if (this.#drafts.get(id)?.generation !== generation) {
      return false;
    }
    this.#drafts.delete(id);
    return true;
  }
}

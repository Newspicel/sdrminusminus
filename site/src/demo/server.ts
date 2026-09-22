import type { DemoSession, RecordedResponse } from "../../../web/e2e/demoSession";
import type {
  ChannelSettings,
  DeviceSettings,
  StateSnapshot,
  WorkspaceDetail,
  WorkspaceInfo,
  WorkspaceSnapshot,
  WorkspacesResponse,
} from "../../../web/src/lib/types";

const WORKSPACE = /^\/api\/workspaces\/(\d+)$/;
const STEP = /^\/api\/workspaces\/(\d+)\/(undo|redo)$/;
const DEVICE = /^\/api\/devicesets\/(\d+)\/device$/;
const CHANNEL = /^\/api\/devicesets\/(\d+)\/channels\/(\d+)$/;

interface History {
  past: WorkspaceSnapshot[];
  future: WorkspaceSnapshot[];
}

export class DemoServer {
  private readonly recorded = new Map<string, RecordedResponse>();
  private readonly details = new Map<number, WorkspaceDetail>();
  private readonly histories = new Map<number, History>();
  private state: StateSnapshot | null = null;
  private list: WorkspacesResponse | null = null;

  constructor(session: DemoSession) {
    for (const response of session.responses) {
      this.recorded.set(`${response.method} ${response.path}`, response);
      const workspace = WORKSPACE.exec(response.path);
      if (response.method === "GET" && workspace !== null) {
        this.details.set(Number(workspace[1]), JSON.parse(response.body) as WorkspaceDetail);
      }
    }
    this.state = this.json<StateSnapshot>("GET /api/state");
    this.list = this.json<WorkspacesResponse>("GET /api/workspaces");
  }

  handle(method: string, url: URL, body: string | null): Response {
    const path = url.pathname;
    return (
      this.dynamic(method, path, body) ??
      this.replay(`${method} ${path}${url.search}`) ??
      this.replay(`${method} ${path}`) ??
      missing(method)
    );
  }

  private dynamic(method: string, path: string, body: string | null): Response | null {
    if (method === "GET" && path === "/api/state" && this.state !== null) {
      return json(this.state);
    }
    if (method === "GET" && path === "/api/workspaces" && this.list !== null) {
      return json(this.list);
    }
    const workspace = WORKSPACE.exec(path);
    const detail = workspace === null ? undefined : this.details.get(Number(workspace[1]));
    if (detail !== undefined && method === "GET") {
      return json(detail);
    }
    if (detail !== undefined && method === "PUT" && body !== null) {
      return json(this.update(detail, JSON.parse(body)));
    }
    const step = STEP.exec(path);
    if (step !== null && method === "POST") {
      const stepped = this.step(Number(step[1]), step[2] === "undo");
      return stepped === null ? missing(method) : json(stepped);
    }
    const device = DEVICE.exec(path);
    if (device !== null && method === "PATCH" && body !== null) {
      this.patchDevice(Number(device[1]), JSON.parse(body) as DeviceSettings);
      return empty();
    }
    const channel = CHANNEL.exec(path);
    if (channel !== null && method === "PATCH" && body !== null) {
      this.patchChannel(Number(channel[1]), Number(channel[2]), JSON.parse(body));
      return empty();
    }
    return null;
  }

  private update(
    detail: WorkspaceDetail,
    change: { name?: string; snapshot?: WorkspaceSnapshot },
  ): WorkspaceInfo {
    if (change.snapshot !== undefined) {
      const history = this.history(detail.id);
      history.past.push(detail.snapshot);
      history.future = [];
    }
    return this.store(detail, change.snapshot ?? detail.snapshot, change.name ?? detail.name);
  }

  private step(id: number, undo: boolean): WorkspaceDetail | null {
    const detail = this.details.get(id);
    const history = this.history(id);
    const target = undo ? history.past.pop() : history.future.pop();
    if (detail === undefined || target === undefined) {
      return null;
    }
    (undo ? history.future : history.past).push(detail.snapshot);
    this.store(detail, target, detail.name);
    return this.details.get(id) ?? null;
  }

  private store(detail: WorkspaceDetail, snapshot: WorkspaceSnapshot, name: string): WorkspaceInfo {
    const history = this.history(detail.id);
    const next: WorkspaceDetail = {
      ...detail,
      name,
      snapshot,
      revision: detail.revision + 1,
      nodes: snapshot.graph.nodes.length,
      updated_at: new Date().toISOString(),
      history: { can_undo: history.past.length > 0, can_redo: history.future.length > 0 },
    };
    this.details.set(detail.id, next);
    const info: WorkspaceInfo = {
      id: next.id,
      name: next.name,
      nodes: next.nodes,
      revision: next.revision,
      created_at: next.created_at,
      updated_at: next.updated_at,
    };
    if (this.list !== null) {
      this.list = {
        ...this.list,
        workspaces: this.list.workspaces.map((entry) =>
          entry.id === info.id ? { ...entry, ...info } : entry,
        ),
      };
    }
    return info;
  }

  private history(id: number): History {
    let history = this.histories.get(id);
    if (history === undefined) {
      history = { past: [], future: [] };
      this.histories.set(id, history);
    }
    return history;
  }

  private patchDevice(ds: number, settings: DeviceSettings): void {
    this.editState((set) =>
      set.id === ds ? { ...set, settings: { ...set.settings, ...settings } } : set,
    );
  }

  private patchChannel(ds: number, ch: number, settings: ChannelSettings): void {
    this.editState((set) =>
      set.id === ds
        ? {
            ...set,
            channels: set.channels.map((open) =>
              open.id === ch ? { ...open, settings: { ...open.settings, ...settings } } : open,
            ),
          }
        : set,
    );
  }

  private editState(
    change: (set: StateSnapshot["device_sets"][number]) => StateSnapshot["device_sets"][number],
  ): void {
    if (this.state !== null) {
      this.state = { ...this.state, device_sets: this.state.device_sets.map(change) };
    }
  }

  private json<T>(key: string): T | null {
    const response = this.recorded.get(key);
    return response === undefined ? null : (JSON.parse(response.body) as T);
  }

  private replay(key: string): Response | null {
    const response = this.recorded.get(key);
    return response === undefined
      ? null
      : new Response(response.body, {
          status: response.status,
          headers: { "content-type": response.contentType },
        });
  }
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
  });
}

function empty(): Response {
  return new Response(null, { status: 204 });
}

function missing(method: string): Response {
  return method === "GET"
    ? new Response(JSON.stringify({ error: "Not part of this demo" }), {
        status: 404,
        headers: { "content-type": "application/json" },
      })
    : empty();
}

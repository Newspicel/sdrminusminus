import { describe, expect, it } from "vitest";
import type { DemoSession, RecordedResponse } from "../../../web/e2e/demoSession";
import { DemoServer } from "./server";

function recorded(path: string, body: unknown): RecordedResponse {
  return {
    method: "GET",
    path,
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(body),
  };
}

const SNAPSHOT = { version: 3, graph: { nodes: [], edges: [] } };
const DETAIL = {
  id: 2,
  name: "Demo",
  nodes: 0,
  revision: 5,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
  snapshot: SNAPSHOT,
};
const STATE = {
  device_sets: [
    { id: 0, settings: { center_hz: 100 }, channels: [{ id: 1, settings: { frequency_hz: 5 } }] },
  ],
};

function server(): DemoServer {
  const session: DemoSession = {
    scene: "test",
    title: "test",
    speaker: null,
    loopMs: 1000,
    greeting: [],
    streams: [],
    events: [],
    responses: [
      recorded("/api/state", STATE),
      recorded("/api/workspaces", { workspaces: [{ ...DETAIL, snapshot: undefined }] }),
      recorded("/api/workspaces/2", DETAIL),
      recorded("/api/decoderlog?limit=500", { records: ["a"] }),
    ],
  };
  return new DemoServer(session);
}

async function call(target: DemoServer, method: string, path: string, body?: unknown) {
  const response = target.handle(
    method,
    new URL(path, "http://demo"),
    body === undefined ? null : JSON.stringify(body),
  );
  return { status: response.status, body: response.status === 204 ? null : await response.json() };
}

describe("DemoServer", () => {
  it("replays a recorded GET with its query", async () => {
    expect((await call(server(), "GET", "/api/decoderlog?limit=500")).body).toEqual({
      records: ["a"],
    });
  });

  it("answers an unrecorded GET with an API error", async () => {
    const answer = await call(server(), "GET", "/api/calls");
    expect(answer.status).toBe(404);
    expect(typeof answer.body.error).toBe("string");
  });

  it("keeps a saved workspace and bumps its revision", async () => {
    const target = server();
    const moved = { ...SNAPSHOT, settings: { band_ruler: true } };
    const info = await call(target, "PUT", "/api/workspaces/2", { revision: 5, snapshot: moved });
    expect(info.body.revision).toBe(6);
    const detail = await call(target, "GET", "/api/workspaces/2");
    expect(detail.body.snapshot).toEqual(moved);
    expect(detail.body.history.can_undo).toBe(true);
    const list = await call(target, "GET", "/api/workspaces");
    expect(list.body.workspaces[0].revision).toBe(6);
  });

  it("undoes and redoes a save", async () => {
    const target = server();
    const moved = { ...SNAPSHOT, settings: { band_ruler: true } };
    await call(target, "PUT", "/api/workspaces/2", { revision: 5, snapshot: moved });
    expect((await call(target, "POST", "/api/workspaces/2/undo")).body.snapshot).toEqual(SNAPSHOT);
    expect((await call(target, "POST", "/api/workspaces/2/redo")).body.snapshot).toEqual(moved);
  });

  it("merges a retune into the state", async () => {
    const target = server();
    await call(target, "PATCH", "/api/devicesets/0/device", { center_hz: 200 });
    await call(target, "PATCH", "/api/devicesets/0/channels/1", { frequency_hz: 9 });
    const state = await call(target, "GET", "/api/state");
    expect(state.body.device_sets[0].settings.center_hz).toBe(200);
    expect(state.body.device_sets[0].channels[0].settings.frequency_hz).toBe(9);
  });

  it("accepts other writes without a body", async () => {
    expect((await call(server(), "POST", "/api/bookmarks", {})).status).toBe(204);
  });
});

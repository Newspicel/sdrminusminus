import type { DemoSession } from "../../../web/e2e/demoSession";
import { sceneFrom } from "./scenes";
import { DemoServer } from "./server";
import { demoSocketClass } from "./socket";

const passThrough = globalThis.fetch.bind(globalThis);

const session: Promise<DemoSession> = passThrough(`/demo/${sceneFrom(location.search)}.json`).then(
  (response) => {
    if (!response.ok) {
      throw new Error(`demo session: HTTP ${response.status}`);
    }
    return response.json() as Promise<DemoSession>;
  },
);
const server = session.then((loaded) => new DemoServer(loaded));

globalThis.fetch = async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
  const request = new Request(input, init);
  const url = new URL(request.url);
  if (url.origin !== location.origin || !url.pathname.startsWith("/api/")) {
    return passThrough(input, init);
  }
  const body = request.method === "GET" || request.method === "HEAD" ? null : await request.text();
  return (await server).handle(request.method, url, body);
};

globalThis.WebSocket = demoSocketClass(session) as unknown as typeof WebSocket;

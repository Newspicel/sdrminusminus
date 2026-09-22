import { mkdir, writeFile } from "node:fs/promises";
import { type Page, type Response, test } from "@playwright/test";
import {
  type DemoSession,
  type RecordedMessage,
  type RecordedResponse,
  type RecordedStream,
  subscriptionKey,
} from "./demoSession";
import { listen, type Scene, scene } from "./scenes";

const OUT = "../site/public/demo";
const LOOP_MS = 12_000;
const LEAD_MS = 3_000;
const RECORDED = ["rds", "adsb", "ais", "pocsag", "ft8"];

const STARTED: Record<string, string> = {
  StreamStarted: "Spectrum",
  AudioStreamStarted: "Audio",
  VideoStreamStarted: "Video",
  IqStreamStarted: "Iq",
  SymbolStreamStarted: "Symbols",
  SurfaceStreamStarted: "Surface",
};
const GREETING = new Set(["Hello", "DecodedBacklog"]);
const DECODED_PER_SECOND = 20;

interface Sent {
  at: number;
  type: string;
  data: Record<string, unknown>;
}

interface Received {
  at: number;
  text?: string;
  binary?: Uint8Array;
}

interface ServerText {
  type: string;
  data?: Record<string, unknown>;
}

class Tape {
  private readonly t0 = Date.now();
  private readonly sent: Sent[] = [];
  private readonly received: Received[] = [];
  private readonly responses = new Map<string, Promise<RecordedResponse | null>>();

  async attach(page: Page): Promise<void> {
    await page.routeWebSocket("**/api/ws", (client) => {
      const server = client.connectToServer();
      client.onMessage((message) => {
        server.send(message);
        if (typeof message === "string") {
          this.sent.push({ at: this.now(), ...JSON.parse(message) });
        }
      });
      server.onMessage((message) => {
        client.send(message);
        this.received.push(
          typeof message === "string"
            ? { at: this.now(), text: message }
            : { at: this.now(), binary: message },
        );
      });
    });
    page.on("response", (response) => {
      const url = new URL(response.url());
      if (url.pathname.startsWith("/api/") && url.pathname !== "/api/ws") {
        const method = response.request().method();
        this.responses.set(`${method} ${url.pathname}${url.search}`, capture(response));
      }
    });
  }

  async session(recorded: Scene): Promise<DemoSession> {
    const end = this.now();
    const from = end - LOOP_MS;
    const streams = this.streams();
    for (const message of this.received) {
      if (message.binary === undefined || message.at < from) {
        continue;
      }
      streams
        .get(streamIdOf(message.binary))
        ?.frames.push({ at: message.at - from, binary: base64(message.binary) });
    }
    const texts = this.received.filter((message) => message.text !== undefined);
    const responses = (await Promise.all(this.responses.values())).filter(
      (response): response is RecordedResponse => response !== null,
    );
    return {
      scene: recorded.id,
      title: recorded.title,
      speaker: recorded.speaker ?? null,
      loopMs: LOOP_MS,
      responses,
      greeting: texts
        .filter((message) => GREETING.has(parse(message).type))
        .map((message) => message.text ?? ""),
      streams: [...new Set(streams.values())].filter((stream) => stream.frames.length > 0),
      events: thinned(texts.filter((message) => message.at >= from && !isControl(message))).map(
        (message): RecordedMessage => ({ at: message.at - from, text: message.text }),
      ),
    };
  }

  private streams(): Map<number, RecordedStream> {
    const byId = new Map<number, RecordedStream>();
    for (const message of this.received) {
      const event = message.text === undefined ? null : parse(message);
      const kind = event === null ? undefined : STARTED[event.type];
      if (event === null || kind === undefined) {
        continue;
      }
      const command = this.commandFor(kind, event, message.at);
      const key = command === null ? null : subscriptionKey(command);
      const id = event.data?.stream_id;
      if (key !== null && typeof id === "number") {
        byId.set(id, { key, started: message.text ?? "", frames: [] });
      }
    }
    return byId;
  }

  private commandFor(kind: string, event: ServerText, before: number): Sent | null {
    const matching = this.sent.filter(
      (command) =>
        command.at <= before &&
        command.type === `Subscribe${kind}` &&
        Object.entries(command.data).every(
          ([name, value]) =>
            name === "fps" || name === "bins" || (event.data?.[name] ?? 0) === (value ?? 0),
        ),
    );
    return matching.at(-1) ?? null;
  }

  private now(): number {
    return Date.now() - this.t0;
  }
}

function streamIdOf(frame: Uint8Array): number {
  return new DataView(frame.buffer, frame.byteOffset, frame.byteLength).getUint16(2, true);
}

function base64(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) {
    text += String.fromCharCode(byte);
  }
  return btoa(text);
}

function parse(message: Received): ServerText {
  return JSON.parse(message.text ?? "{}") as ServerText;
}

function thinned(messages: Received[]): Received[] {
  const perSecond = new Map<number, number>();
  return messages.filter((message) => {
    if (parse(message).type !== "Decoded") {
      return true;
    }
    const second = Math.floor(message.at / 1000);
    const count = perSecond.get(second) ?? 0;
    perSecond.set(second, count + 1);
    return count < DECODED_PER_SECOND;
  });
}

function isControl(message: Received): boolean {
  const type = parse(message).type;
  return GREETING.has(type) || type in STARTED || type === "StreamStopped";
}

async function capture(response: Response): Promise<RecordedResponse | null> {
  const contentType = response.headers()["content-type"] ?? "";
  if (!contentType.includes("json") && !contentType.startsWith("text/")) {
    return null;
  }
  const url = new URL(response.url());
  try {
    return {
      method: response.request().method(),
      path: `${url.pathname}${url.search}`,
      status: response.status(),
      contentType,
      body: await response.text(),
    };
  } catch {
    return null;
  }
}

for (const id of RECORDED) {
  test(`record the ${id} demo`, async ({ page }) => {
    const recorded = scene(id);
    await page.goto("/");
    await recorded.stage(page);
    await recorded.ready(page);

    const tape = new Tape();
    await tape.attach(page);
    await page.reload();
    await recorded.ready(page);
    if (recorded.speaker !== undefined) {
      await listen(page, recorded.speaker);
    }
    await page.waitForTimeout(LOOP_MS + LEAD_MS);

    await mkdir(OUT, { recursive: true });
    await writeFile(`${OUT}/${id}.json`, JSON.stringify(await tape.session(recorded)));
  });
}

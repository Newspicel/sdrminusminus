export interface RecordedResponse {
  method: string;
  path: string;
  status: number;
  contentType: string;
  body: string;
}

export interface RecordedMessage {
  at: number;
  text?: string;
  binary?: string;
}

export interface RecordedStream {
  key: string;
  started: string;
  frames: RecordedMessage[];
}

export interface DemoSession {
  scene: string;
  title: string;
  speaker: string | null;
  loopMs: number;
  responses: RecordedResponse[];
  greeting: string[];
  streams: RecordedStream[];
  events: RecordedMessage[];
}

interface Command {
  type: string;
  data?: Record<string, unknown>;
}

const SUBSCRIPTION = /^(?:Un)?[Ss]ubscribe([A-Za-z]+)$/;
const VOLATILE = new Set(["fps", "bins"]);

export function subscriptionKey(command: Command): string | null {
  const kind = SUBSCRIPTION.exec(command.type)?.[1];
  if (kind === undefined || kind === "Diagnostics") {
    return null;
  }
  const fields = Object.entries(command.data ?? {})
    .filter(([name]) => !VOLATILE.has(name))
    .map(([name, value]) => [name, value ?? 0] as const)
    .toSorted(([a], [b]) => a.localeCompare(b));
  return `${kind}:${JSON.stringify(fields)}`;
}

export function isUnsubscribe(command: Command): boolean {
  return command.type.startsWith("Unsubscribe");
}

// Typed REST access. `openapi-fetch` gives full inference from the generated schema; the thin
// query helpers key TanStack Query by path so WS `StateChanged` events can invalidate them
// (PLAN §4 step 4, §10). No polling — invalidation is WS-driven.
import { queryOptions } from "@tanstack/react-query";
import createClient from "openapi-fetch";
import type { paths } from "../generated/schema";
import { getToken, withToken } from "./auth";
import type {
  ApiError,
  AuthInfo,
  Bookmark,
  ChannelSettings,
  ChannelTypesResponse,
  ClientsResponse,
  CreateBookmarkRequest,
  DecoderLogFilter,
  DecoderLogResponse,
  DeviceSettings,
  DevicesResponse,
  DoctorReport,
  ExportFormat,
  PresetInfo,
  RecordAction,
  RecordingStatus,
  RecordingsResponse,
  ScannerStatus,
  ScanSettings,
  StateSnapshot,
  TemplatesResponse,
} from "./types";

export const client = createClient<paths>({ baseUrl: "/" });

// One middleware carries the shared token on every request (PLAN §12). Read per request, not
// captured once: the token is entered after the client module has already been imported.
client.use({
  onRequest({ request }) {
    const token = getToken();
    if (token !== null) {
      request.headers.set("Authorization", `Bearer ${token}`);
    }
    return request;
  },
});

export const STATE_KEY = ["get", "/api/state"] as const;
export const DEVICES_KEY = ["get", "/api/devices"] as const;
export const CHANNEL_TYPES_KEY = ["get", "/api/channeltypes"] as const;
export const PRESETS_KEY = ["get", "/api/presets"] as const;
export const BOOKMARKS_KEY = ["get", "/api/bookmarks"] as const;
export const RECORDINGS_KEY = ["get", "/api/recordings"] as const;
export const DECODER_LOG_KEY = ["get", "/api/decoderlog"] as const;
export const TEMPLATES_KEY = ["get", "/api/templates"] as const;
export const AUTH_KEY = ["get", "/api/auth"] as const;
export const CLIENTS_KEY = ["get", "/api/clients"] as const;
export const DOCTOR_KEY = ["get", "/api/doctor"] as const;

export function stateQuery() {
  return queryOptions({
    queryKey: STATE_KEY,
    queryFn: async (): Promise<StateSnapshot> => unwrap(await client.GET("/api/state")),
  });
}

export function devicesQuery() {
  return queryOptions({
    queryKey: DEVICES_KEY,
    queryFn: async (): Promise<DevicesResponse> => unwrap(await client.GET("/api/devices")),
  });
}

export async function createDeviceSet(deviceId: string): Promise<number> {
  return unwrap(
    await client.POST("/api/devicesets", {
      body: { device_id: deviceId },
    }),
  ).id;
}

export async function deleteDeviceSet(ds: number): Promise<void> {
  unwrap(
    await client.DELETE("/api/devicesets/{ds}", {
      params: { path: { ds } },
    }),
  );
}

export async function patchDevice(ds: number, settings: DeviceSettings): Promise<void> {
  unwrap(
    await client.PATCH("/api/devicesets/{ds}/device", {
      params: { path: { ds } },
      body: settings,
    }),
  );
}

export function channelTypesQuery() {
  return queryOptions({
    queryKey: CHANNEL_TYPES_KEY,
    queryFn: async (): Promise<ChannelTypesResponse> =>
      unwrap(await client.GET("/api/channeltypes")),
  });
}

export async function createChannel(ds: number, settings: ChannelSettings): Promise<number> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/channels", {
      params: { path: { ds } },
      body: { settings },
    }),
  ).id;
}

export async function patchChannel(
  ds: number,
  ch: number,
  settings: ChannelSettings,
): Promise<void> {
  unwrap(
    await client.PATCH("/api/devicesets/{ds}/channels/{ch}", {
      params: { path: { ds, ch } },
      body: settings,
    }),
  );
}

export async function deleteChannel(ds: number, ch: number): Promise<void> {
  unwrap(
    await client.DELETE("/api/devicesets/{ds}/channels/{ch}", {
      params: { path: { ds, ch } },
    }),
  );
}

export function presetsQuery() {
  return queryOptions({
    queryKey: PRESETS_KEY,
    queryFn: async (): Promise<PresetInfo[]> => unwrap(await client.GET("/api/presets")),
  });
}

export async function createPreset(name: string, ds: number): Promise<number> {
  return unwrap(
    await client.POST("/api/presets", {
      body: { name, device_set: ds },
    }),
  ).id;
}

export async function applyPreset(id: number, ds: number): Promise<void> {
  unwrap(
    await client.POST("/api/presets/{id}/apply", {
      params: { path: { id } },
      body: { device_set: ds },
    }),
  );
}

export async function deletePreset(id: number): Promise<void> {
  unwrap(
    await client.DELETE("/api/presets/{id}", {
      params: { path: { id } },
    }),
  );
}

export function bookmarksQuery() {
  return queryOptions({
    queryKey: BOOKMARKS_KEY,
    queryFn: async (): Promise<Bookmark[]> => unwrap(await client.GET("/api/bookmarks")),
  });
}

export async function createBookmark(bookmark: CreateBookmarkRequest): Promise<number> {
  return unwrap(
    await client.POST("/api/bookmarks", {
      body: bookmark,
    }),
  ).id;
}

export async function deleteBookmark(id: number): Promise<void> {
  unwrap(
    await client.DELETE("/api/bookmarks/{id}", {
      params: { path: { id } },
    }),
  );
}

export function recordingsQuery() {
  return queryOptions({
    queryKey: RECORDINGS_KEY,
    queryFn: async (): Promise<RecordingsResponse> => unwrap(await client.GET("/api/recordings")),
  });
}

export async function recordDeviceSet(ds: number, action: RecordAction): Promise<RecordingStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/record", {
      params: { path: { ds } },
      body: { action },
    }),
  );
}

export async function deleteRecording(id: number): Promise<void> {
  unwrap(
    await client.DELETE("/api/recordings/{id}", {
      params: { path: { id } },
    }),
  );
}

export function templatesQuery() {
  return queryOptions({
    queryKey: TEMPLATES_KEY,
    queryFn: async (): Promise<TemplatesResponse> => unwrap(await client.GET("/api/templates")),
    // Built-in and compiled into the server: it can only change across a restart, which
    // reconnects the socket and refetches everything anyway.
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export async function applyTemplate(id: string, ds: number): Promise<void> {
  unwrap(
    await client.POST("/api/templates/{id}/apply", {
      params: { path: { id } },
      body: { device_set: ds },
    }),
  );
}

/** Whether this server wants a token. Answered unauthenticated, so it is the one call that
 * works before the user has supplied one. */
export function authQuery() {
  return queryOptions({
    queryKey: AUTH_KEY,
    queryFn: async (): Promise<AuthInfo> => unwrap(await client.GET("/api/auth")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** How many clients share this server (PLAN §16 M5). Invalidated by the `clients` scope on
 * every connect and disconnect — never polled. */
export function clientsQuery() {
  return queryOptions({
    queryKey: CLIENTS_KEY,
    queryFn: async (): Promise<ClientsResponse> => unwrap(await client.GET("/api/clients")),
  });
}

export function doctorQuery(enabled: boolean) {
  return queryOptions({
    queryKey: DOCTOR_KEY,
    queryFn: async (): Promise<DoctorReport> => unwrap(await client.GET("/api/doctor")),
    enabled,
    // Probing enumerates USB; never do it on a window focus.
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
  });
}

export async function startScan(ds: number, settings: ScanSettings): Promise<ScannerStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/scanner", {
      params: { path: { ds } },
      body: { action: "start", settings },
    }),
  );
}

export async function stopScan(ds: number): Promise<ScannerStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/scanner", {
      params: { path: { ds } },
      body: { action: "stop" },
    }),
  );
}

export function decoderLogQuery(filter: DecoderLogFilter) {
  const query = normalizeFilter(filter);
  return queryOptions({
    // The filter is part of the key so changing it refetches, and it sits under
    // DECODER_LOG_KEY so a `decoder_log` StateChanged invalidates every filter at once.
    queryKey: [...DECODER_LOG_KEY, query] as const,
    queryFn: async (): Promise<DecoderLogResponse> =>
      unwrap(await client.GET("/api/decoderlog", { params: { query } })),
  });
}

export async function clearDecoderLog(filter: DecoderLogFilter): Promise<number> {
  return unwrap(
    await client.DELETE("/api/decoderlog", {
      params: { query: normalizeFilter(filter) },
    }),
  ).deleted;
}

/** A plain href for a download link — the browser must navigate to it so the server's
 * `Content-Disposition` applies, which a fetch through `client` would swallow. `limit` is
 * dropped: the export endpoint ignores it in favour of its own cap. */
export function decoderLogExportUrl(format: ExportFormat, filter: DecoderLogFilter): string {
  const { limit: _limit, ...rest } = normalizeFilter(filter);
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(rest)) {
    params.set(key, String(value));
  }
  const query = params.toString();
  // The token rides in the query here: this href is navigated to by the browser, which cannot
  // be given an Authorization header (and a fetch would swallow `Content-Disposition`).
  return withToken(
    query.length > 0
      ? `/api/decoderlog/export/${format}?${query}`
      : `/api/decoderlog/export/${format}`,
  );
}

/** Blank fields are absent fields: a cleared filter input must not become `q=` (which the
 * server would match on) nor a second query key for what is the same request. */
function normalizeFilter(filter: DecoderLogFilter): DecoderLogFilter {
  const normalized: DecoderLogFilter = {};
  for (const [key, value] of Object.entries(filter)) {
    if (value != null && value !== "") {
      (normalized as Record<string, string | number>)[key] = value;
    }
  }
  return normalized;
}

/** Narrows the `ApiError` contract at runtime: openapi-fetch yields `{ error: undefined }`
 * for an error response with an empty body and a plain string for a non-JSON one — both are
 * what the dev proxy serves while the server is down, so gating on the parsed `error` alone
 * turned a dead backend into `data.id` crashes and silently "successful" deletes. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
  const { data, error, response } = result;
  if (response.ok) {
    return data as T;
  }
  if (isApiError(error)) {
    throw new Error(error.detail ? `${error.error}: ${error.detail}` : error.error);
  }
  const body = typeof error === "string" ? error.trim().slice(0, 200) : "";
  throw new Error(
    body.length > 0
      ? `HTTP ${response.status}: ${body}`
      : `HTTP ${response.status}: no response from the server`,
  );
}

function isApiError(error: unknown): error is ApiError {
  return (
    typeof error === "object" &&
    error !== null &&
    typeof (error as { error?: unknown }).error === "string"
  );
}

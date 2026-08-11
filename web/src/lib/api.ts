// Typed REST access. `openapi-fetch` gives full inference from the generated schema; the thin
// query helpers key TanStack Query by path so WS `StateChanged` events can invalidate them
// (PLAN §4 step 4, §10). No polling — invalidation is WS-driven.
import { queryOptions } from "@tanstack/react-query";
import createClient from "openapi-fetch";
import { migrateSnapshot } from "../canvas/graph";
import type { paths } from "../generated/schema";
import { getToken, rejectToken, withToken } from "./auth";
import type {
  ApiError,
  AuthInfo,
  BandPlan,
  BandRegionMatch,
  BandRegionsResponse,
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
  PatchApplyReport,
  PatchCatalog,
  PresetInfo,
  RecordAction,
  RecordingStatus,
  RecordingsResponse,
  ScannerStatus,
  ScanSettings,
  StateSnapshot,
  TemplatesResponse,
  WorkspaceDetail,
  WorkspaceInfo,
  WorkspaceSnapshot,
  WorkspacesResponse,
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
  onResponse({ response }) {
    // A 401 with a token stored means the token is wrong or the server's changed: drop it so
    // the gate prompts again instead of every request failing forever.
    if (response.status === 401) {
      rejectToken();
    }
    return response;
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
export const WORKSPACES_KEY = ["get", "/api/workspaces"] as const;
export const PATCH_CATALOG_KEY = ["get", "/api/patch/catalog"] as const;
export const BAND_REGIONS_KEY = ["get", "/api/bandplan/regions"] as const;

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

export async function recordDeviceSet(
  ds: number,
  action: RecordAction,
  stream = 0,
): Promise<RecordingStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/record", {
      params: { path: { ds } },
      body: { action, stream },
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
    // Not cached forever any more: the table is compiled in, but `supported_devices` is computed
    // against the radios attached *now*, so plugging one in changes the answer. Invalidated on
    // the `devices` scope alongside the device list itself.
    staleTime: 30_000,
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

/** The workspace switcher's view: every workspace plus the active one (PLAN §10 — the shell is
 * workspace config, so it is server-side and every client converges on it). */
export function workspacesQuery() {
  return queryOptions({
    queryKey: WORKSPACES_KEY,
    queryFn: async (): Promise<WorkspacesResponse> => unwrap(await client.GET("/api/workspaces")),
  });
}

/** One workspace with its layout. Keyed under `WORKSPACES_KEY` so a `workspaces` scope
 * invalidates the list and every open layout together.
 *
 * A workspace stored against an older port table is brought up to today's here, on the way into the
 * cache: every read and every edit then sees one shape, and the first write persists it. */
export function workspaceQuery(id: number | null) {
  return queryOptions({
    queryKey: [...WORKSPACES_KEY, id] as const,
    queryFn: async (): Promise<WorkspaceDetail> => {
      const detail = unwrap(
        await client.GET("/api/workspaces/{id}", {
          params: { path: { id: id ?? 0 } },
        }),
      );
      return { ...detail, snapshot: migrateSnapshot(detail.snapshot) };
    },
    enabled: id !== null,
  });
}

export async function createWorkspace(name: string, snapshot?: WorkspaceSnapshot): Promise<number> {
  return unwrap(
    await client.POST("/api/workspaces", {
      body: { name, ...(snapshot ? { snapshot } : {}) },
    }),
  ).id;
}

/** Rename and/or re-lay-out. `revision` is the one the caller last saw: the server answers 409
 * rather than letting a stale layout overwrite the one another client is arranging. */
export async function updateWorkspace(
  id: number,
  update: { revision: number; name?: string; snapshot?: WorkspaceSnapshot },
): Promise<WorkspaceInfo> {
  return unwrap(
    await client.PUT("/api/workspaces/{id}", {
      params: { path: { id } },
      body: update,
    }),
  );
}

export async function deleteWorkspace(id: number): Promise<void> {
  unwrap(await client.DELETE("/api/workspaces/{id}", { params: { path: { id } } }));
}

export async function activateWorkspace(id: number): Promise<void> {
  unwrap(await client.POST("/api/workspaces/{id}/activate", { params: { path: { id } } }));
}

/** Bring the engine up to what the workspace draws (CANVAS §2): open the radios its device nodes
 * name, add the channels it wires. Additive and idempotent, so it is safe on every load. */
export async function applyWorkspace(id: number): Promise<PatchApplyReport> {
  return unwrap(await client.POST("/api/workspaces/{id}/apply", { params: { path: { id } } }));
}

/** The node palette and its ports (CANVAS §1). Static for a build, like the channel types, so
 * it is fetched once and never invalidated. */
export function patchCatalogQuery() {
  return queryOptions({
    queryKey: PATCH_CATALOG_KEY,
    queryFn: async (): Promise<PatchCatalog> => unwrap(await client.GET("/api/patch/catalog")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** The regions a band plan can be read in. Compiled into the server like the node palette. */
export function bandRegionsQuery() {
  return queryOptions({
    queryKey: BAND_REGIONS_KEY,
    queryFn: async (): Promise<BandRegionsResponse> =>
      unwrap(await client.GET("/api/bandplan/regions")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** One region's whole allocation table, already layered. Fetched once and then clipped and
 * searched locally (`components/bandPlan.ts`): a scope pans and zooms continuously, and a
 * request per frame to re-cut a document that cannot change would be absurd. */
export function bandPlanQuery(region: string | null) {
  return queryOptions({
    queryKey: ["get", "/api/bandplan/regions/{region}", region] as const,
    queryFn: async (): Promise<BandPlan> =>
      unwrap(
        await client.GET("/api/bandplan/regions/{region}", {
          params: { path: { region: region ?? "" } },
        }),
      ),
    enabled: region !== null,
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/** Which region a coordinate falls in. Not a query: it is asked once, when the operator presses
 * "detect", and its answer is a suggestion they then confirm. */
export async function locateBandRegion(lat: number, lon: number): Promise<BandRegionMatch> {
  return unwrap(await client.GET("/api/bandplan/locate", { params: { query: { lat, lon } } }));
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

import { keepPreviousData, queryOptions } from "@tanstack/react-query";
import createClient from "openapi-fetch";
import { migrateSnapshot } from "../canvas/graph";
import type { paths } from "../generated/schema";
import { getToken, rejectToken, withToken } from "./auth";
import type {
  AboutResponse,
  ApiError,
  AudioRecordingStatus,
  AudioRecordingsResponse,
  AuthInfo,
  BandPlan,
  BandRegionsResponse,
  Bookmark,
  CapturedImagesResponse,
  ChannelSettings,
  ChannelTypesResponse,
  CreateBookmarkRequest,
  DecoderLogFilter,
  DecoderLogResponse,
  DeviceSettings,
  DevicesResponse,
  DoctorReport,
  ExportFormat,
  HuntSettings,
  HuntStatus,
  IonosondeReport,
  LicenseTextResponse,
  NetworkExportAction,
  NetworkExportSettings,
  NetworkExportStatus,
  NmeaDevicesResponse,
  OccupancyReport,
  PatchApplyReport,
  PatchCatalog,
  PlaybackAction,
  PlaybackStatus,
  PresetInfo,
  RecordAction,
  RecordingFormat,
  RecordingStatus,
  RecordingsResponse,
  ScannerStatus,
  ScanSessionStatus,
  ScanSettings,
  StateSnapshot,
  TemplatesResponse,
  TimeMachineAction,
  TimeMachineNode,
  TimeMachineStatus,
  ToolRequest,
  ToolResponse,
  ToolsResponse,
  VoiceCallsResponse,
  WorkspaceDetail,
  WorkspaceInfo,
  WorkspaceSnapshot,
  WorkspacesResponse,
} from "./types";

export const client = createClient<paths>({ baseUrl: "/" });

client.use({
  onRequest({ request }) {
    const token = getToken();
    if (token !== null) {
      request.headers.set("Authorization", `Bearer ${token}`);
    }
    return request;
  },
  onResponse({ response }) {
    if (response.status === 401) {
      rejectToken();
    }
    return response;
  },
});

export const STATE_KEY = ["get", "/api/state"] as const;
export const DEVICES_KEY = ["get", "/api/devices"] as const;
export const NMEA_DEVICES_KEY = ["get", "/api/position/nmea-devices"] as const;
export const CHANNEL_TYPES_KEY = ["get", "/api/channeltypes"] as const;
export const PRESETS_KEY = ["get", "/api/presets"] as const;
export const BOOKMARKS_KEY = ["get", "/api/bookmarks"] as const;
export const RECORDINGS_KEY = ["get", "/api/recordings"] as const;
export const AUDIO_RECORDINGS_KEY = ["get", "/api/audiorecordings"] as const;
export const CALLS_KEY = ["get", "/api/calls"] as const;
export const IMAGES_KEY = ["get", "/api/images"] as const;
export const DECODER_LOG_KEY = ["get", "/api/decoderlog"] as const;
export const TEMPLATES_KEY = ["get", "/api/templates"] as const;
export const AUTH_KEY = ["get", "/api/auth"] as const;
export const DOCTOR_KEY = ["get", "/api/doctor"] as const;
export const OCCUPANCY_KEY = ["get", "/api/occupancy"] as const;
export const IONOSONDE_KEY = ["get", "/api/ionosonde"] as const;
export const ABOUT_KEY = ["get", "/api/about"] as const;
export const WORKSPACES_KEY = ["get", "/api/workspaces"] as const;
export const PATCH_CATALOG_KEY = ["get", "/api/patch/catalog"] as const;
export const BAND_REGIONS_KEY = ["get", "/api/bandplan/regions"] as const;
export const TOOLS_KEY = ["get", "/api/tools"] as const;
export const TOOL_RUN_KEY = ["post", "/api/tools/run"] as const;

export function stateQuery() {
  return queryOptions({
    queryKey: STATE_KEY,
    queryFn: async (): Promise<StateSnapshot> => unwrap(await client.GET("/api/state")),
  });
}

export function callsQuery() {
  return queryOptions({
    queryKey: CALLS_KEY,
    queryFn: async (): Promise<VoiceCallsResponse> => unwrap(await client.GET("/api/calls")),
  });
}

export function callAudioUrl(url: string): string {
  return withToken(url);
}

export function imagesQuery() {
  return queryOptions({
    queryKey: IMAGES_KEY,
    queryFn: async (): Promise<CapturedImagesResponse> => unwrap(await client.GET("/api/images")),
  });
}

export function capturedImageUrl(url: string): string {
  return withToken(url);
}

export function occupancyQuery(minSamples: number) {
  return queryOptions({
    queryKey: [...OCCUPANCY_KEY, minSamples] as const,
    queryFn: async (): Promise<OccupancyReport> =>
      unwrap(
        await client.GET("/api/occupancy", { params: { query: { min_samples: minSamples } } }),
      ),
    refetchInterval: 15_000,
  });
}

export function ionosondeQuery(enabled: boolean) {
  return queryOptions({
    queryKey: IONOSONDE_KEY,
    queryFn: async (): Promise<IonosondeReport> => unwrap(await client.GET("/api/ionosonde")),
    enabled,
    staleTime: 10 * 60_000,
    refetchInterval: 15 * 60_000,
  });
}

export function devicesQuery() {
  return queryOptions({
    queryKey: DEVICES_KEY,
    queryFn: async (): Promise<DevicesResponse> => unwrap(await client.GET("/api/devices")),
  });
}

export function nmeaDevicesQuery() {
  return queryOptions({
    queryKey: NMEA_DEVICES_KEY,
    queryFn: async (): Promise<NmeaDevicesResponse> =>
      unwrap(await client.GET("/api/position/nmea-devices")),
    staleTime: 5_000,
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

export async function createPreset(name: string): Promise<number> {
  return unwrap(await client.POST("/api/presets", { body: { name } })).id;
}

export async function applyPreset(id: number): Promise<void> {
  unwrap(await client.POST("/api/presets/{id}/apply", { params: { path: { id } } }));
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

export function audioRecordingsQuery() {
  return queryOptions({
    queryKey: AUDIO_RECORDINGS_KEY,
    queryFn: async (): Promise<AudioRecordingsResponse> =>
      unwrap(await client.GET("/api/audiorecordings")),
  });
}

export async function recordChannelAudio(
  ds: number,
  ch: number,
  action: RecordAction,
): Promise<AudioRecordingStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/channels/{ch}/record", {
      params: { path: { ds, ch } },
      body: { action },
    }),
  );
}

export function audioRecordingDownloadUrl(file: string): string {
  return withToken(`/api/audiorecordings/${encodeURIComponent(file)}/download`);
}

export async function deleteAudioRecording(file: string): Promise<void> {
  unwrap(
    await client.DELETE("/api/audiorecordings/{file}", {
      params: { path: { file } },
    }),
  );
}

export async function recordChannelBaseband(
  ds: number,
  ch: number,
  action: RecordAction,
): Promise<RecordingStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/channels/{ch}/baseband", {
      params: { path: { ds, ch } },
      body: { action },
    }),
  );
}

export async function networkExportChannel(
  ds: number,
  ch: number,
  action: NetworkExportAction,
  node: string,
  settings: NetworkExportSettings,
): Promise<NetworkExportStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/channels/{ch}/network-export", {
      params: { path: { ds, ch } },
      body: { action, node, settings },
    }),
  );
}

export async function controlTimeMachine(
  ds: number,
  action: TimeMachineAction,
  node: string,
  stream: number,
  settings: TimeMachineNode,
): Promise<TimeMachineStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/time-machine", {
      params: { path: { ds } },
      body: { action, node, stream, settings },
    }),
  );
}

export async function networkExportDeviceSet(
  ds: number,
  action: NetworkExportAction,
  node: string,
  stream: number,
  settings: NetworkExportSettings,
): Promise<NetworkExportStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/network-export", {
      params: { path: { ds } },
      body: { action, node, stream, settings },
    }),
  );
}

export async function controlPlayback(
  ds: number,
  action: PlaybackAction,
  positionSamples?: number,
): Promise<PlaybackStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/playback", {
      params: { path: { ds } },
      body: { action, position_samples: positionSamples },
    }),
  );
}

export function recordingDownloadUrl(id: number, format: RecordingFormat): string {
  const path = `/api/recordings/${id}/download`;
  return withToken(format === "sigmf" ? path : `${path}?format=${format}`);
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

export function authQuery() {
  return queryOptions({
    queryKey: AUTH_KEY,
    queryFn: async (): Promise<AuthInfo> => unwrap(await client.GET("/api/auth")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export async function serverReachable(): Promise<boolean> {
  try {
    return (await client.GET("/api/auth")).response.ok;
  } catch {
    return false;
  }
}

export function workspacesQuery() {
  return queryOptions({
    queryKey: WORKSPACES_KEY,
    queryFn: async (): Promise<WorkspacesResponse> => unwrap(await client.GET("/api/workspaces")),
  });
}

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

export async function applyWorkspace(id: number): Promise<PatchApplyReport> {
  return unwrap(await client.POST("/api/workspaces/{id}/apply", { params: { path: { id } } }));
}

export async function stepWorkspace(id: number, step: "undo" | "redo"): Promise<WorkspaceDetail> {
  const detail = unwrap(
    step === "undo"
      ? await client.POST("/api/workspaces/{id}/undo", { params: { path: { id } } })
      : await client.POST("/api/workspaces/{id}/redo", { params: { path: { id } } }),
  );
  return { ...detail, snapshot: migrateSnapshot(detail.snapshot) };
}

export function patchCatalogQuery() {
  return queryOptions({
    queryKey: PATCH_CATALOG_KEY,
    queryFn: async (): Promise<PatchCatalog> => unwrap(await client.GET("/api/patch/catalog")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export function bandRegionsQuery() {
  return queryOptions({
    queryKey: BAND_REGIONS_KEY,
    queryFn: async (): Promise<BandRegionsResponse> =>
      unwrap(await client.GET("/api/bandplan/regions")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

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

export function toolsQuery() {
  return queryOptions({
    queryKey: TOOLS_KEY,
    queryFn: async (): Promise<ToolsResponse> => unwrap(await client.GET("/api/tools")),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

export function toolRunQuery(request: ToolRequest | null) {
  return queryOptions({
    queryKey: [...TOOL_RUN_KEY, request] as const,
    queryFn: async (): Promise<ToolResponse> =>
      unwrap(await client.POST("/api/tools/run", { body: request as ToolRequest })),
    enabled: request !== null,
    staleTime: Number.POSITIVE_INFINITY,
    placeholderData: keepPreviousData,
  });
}

export async function runTool(request: ToolRequest): Promise<ToolResponse> {
  return unwrap(await client.POST("/api/tools/run", { body: request }));
}

export function aboutQuery(enabled: boolean) {
  return queryOptions({
    queryKey: ABOUT_KEY,
    queryFn: async (): Promise<AboutResponse> => unwrap(await client.GET("/api/about")),
    enabled,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
  });
}

export function licenseTextQuery(id: string | null) {
  return queryOptions({
    queryKey: ["get", "/api/about/licenses", id] as const,
    queryFn: async (): Promise<LicenseTextResponse> =>
      unwrap(await client.GET("/api/about/licenses/{id}", { params: { path: { id: id ?? "" } } })),
    enabled: id !== null,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
  });
}

export function doctorQuery(enabled: boolean) {
  return queryOptions({
    queryKey: DOCTOR_KEY,
    queryFn: async (): Promise<DoctorReport> => unwrap(await client.GET("/api/doctor")),
    enabled,
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

export async function startScanSession(
  deviceSets: readonly number[],
  settings: ScanSettings,
): Promise<ScanSessionStatus> {
  return unwrap(
    await client.POST("/api/scanner", {
      body: { action: "start", device_sets: [...deviceSets], settings },
    }),
  );
}

export async function stopScanSession(): Promise<ScanSessionStatus> {
  return unwrap(await client.POST("/api/scanner", { body: { action: "stop" } }));
}

export async function startHunt(ds: number, settings: HuntSettings): Promise<HuntStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/hunt", {
      params: { path: { ds } },
      body: { action: "start", settings },
    }),
  );
}

export async function stopHunt(ds: number): Promise<HuntStatus> {
  return unwrap(
    await client.POST("/api/devicesets/{ds}/hunt", {
      params: { path: { ds } },
      body: { action: "stop" },
    }),
  );
}

export function decoderLogQuery(filter: DecoderLogFilter) {
  const query = normalizeFilter(filter);
  return queryOptions({
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

export function decoderLogExportUrl(format: ExportFormat, filter: DecoderLogFilter): string {
  const { limit: _limit, ...rest } = normalizeFilter(filter);
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(rest)) {
    params.set(key, String(value));
  }
  const query = params.toString();
  return withToken(
    query.length > 0
      ? `/api/decoderlog/export/${format}?${query}`
      : `/api/decoderlog/export/${format}`,
  );
}

const SCOPE_FIELDS: ReadonlySet<string> = new Set(["nodes", "sources"]);

function normalizeFilter(filter: DecoderLogFilter): DecoderLogFilter {
  const normalized: DecoderLogFilter = {};
  for (const [key, value] of Object.entries(filter)) {
    if (value != null && (value !== "" || SCOPE_FIELDS.has(key))) {
      (normalized as Record<string, string | number>)[key] = value;
    }
  }
  return normalized;
}

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

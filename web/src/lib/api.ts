// Typed REST access. `openapi-fetch` gives full inference from the generated schema; the thin
// query helpers key TanStack Query by path so WS `StateChanged` events can invalidate them
// (PLAN §4 step 4, §10). No polling — invalidation is WS-driven.
import { queryOptions } from "@tanstack/react-query";
import createClient from "openapi-fetch";
import type { paths } from "../generated/schema";
import type {
  ApiError,
  Bookmark,
  ChannelSettings,
  ChannelTypesResponse,
  CreateBookmarkRequest,
  DeviceSettings,
  DevicesResponse,
  PresetInfo,
  StateSnapshot,
} from "./types";

export const client = createClient<paths>({ baseUrl: "/" });

export const STATE_KEY = ["get", "/api/state"] as const;
export const DEVICES_KEY = ["get", "/api/devices"] as const;
export const CHANNEL_TYPES_KEY = ["get", "/api/channeltypes"] as const;
export const PRESETS_KEY = ["get", "/api/presets"] as const;
export const BOOKMARKS_KEY = ["get", "/api/bookmarks"] as const;

export function stateQuery() {
  return queryOptions({
    queryKey: STATE_KEY,
    queryFn: async (): Promise<StateSnapshot> => {
      const { data, error } = await client.GET("/api/state");
      if (error) {
        throw asError(error);
      }
      return data;
    },
  });
}

export function devicesQuery() {
  return queryOptions({
    queryKey: DEVICES_KEY,
    queryFn: async (): Promise<DevicesResponse> => {
      const { data, error } = await client.GET("/api/devices");
      if (error) {
        throw asError(error);
      }
      return data;
    },
  });
}

export async function createDeviceSet(deviceId: string): Promise<number> {
  const { data, error } = await client.POST("/api/devicesets", {
    body: { device_id: deviceId },
  });
  if (error) {
    throw asError(error);
  }
  return data.id;
}

export async function deleteDeviceSet(ds: number): Promise<void> {
  const { error } = await client.DELETE("/api/devicesets/{ds}", {
    params: { path: { ds } },
  });
  if (error) {
    throw asError(error);
  }
}

export async function patchDevice(ds: number, settings: DeviceSettings): Promise<void> {
  const { error } = await client.PATCH("/api/devicesets/{ds}/device", {
    params: { path: { ds } },
    body: settings,
  });
  if (error) {
    throw asError(error);
  }
}

export function channelTypesQuery() {
  return queryOptions({
    queryKey: CHANNEL_TYPES_KEY,
    queryFn: async (): Promise<ChannelTypesResponse> => {
      const { data, error } = await client.GET("/api/channeltypes");
      if (error) {
        throw asError(error);
      }
      return data;
    },
  });
}

export async function createChannel(ds: number, settings: ChannelSettings): Promise<number> {
  const { data, error } = await client.POST("/api/devicesets/{ds}/channels", {
    params: { path: { ds } },
    body: { settings },
  });
  if (error) {
    throw asError(error);
  }
  return data.id;
}

export async function patchChannel(
  ds: number,
  ch: number,
  settings: ChannelSettings,
): Promise<void> {
  const { error } = await client.PATCH("/api/devicesets/{ds}/channels/{ch}", {
    params: { path: { ds, ch } },
    body: settings,
  });
  if (error) {
    throw asError(error);
  }
}

export async function deleteChannel(ds: number, ch: number): Promise<void> {
  const { error } = await client.DELETE("/api/devicesets/{ds}/channels/{ch}", {
    params: { path: { ds, ch } },
  });
  if (error) {
    throw asError(error);
  }
}

export function presetsQuery() {
  return queryOptions({
    queryKey: PRESETS_KEY,
    queryFn: async (): Promise<PresetInfo[]> => {
      const { data, error } = await client.GET("/api/presets");
      if (error) {
        throw asError(error);
      }
      return data;
    },
  });
}

export async function createPreset(name: string, ds: number): Promise<number> {
  const { data, error } = await client.POST("/api/presets", {
    body: { name, device_set: ds },
  });
  if (error) {
    throw asError(error);
  }
  return data.id;
}

export async function applyPreset(id: number, ds: number): Promise<void> {
  const { error } = await client.POST("/api/presets/{id}/apply", {
    params: { path: { id } },
    body: { device_set: ds },
  });
  if (error) {
    throw asError(error);
  }
}

export async function deletePreset(id: number): Promise<void> {
  const { error } = await client.DELETE("/api/presets/{id}", {
    params: { path: { id } },
  });
  if (error) {
    throw asError(error);
  }
}

export function bookmarksQuery() {
  return queryOptions({
    queryKey: BOOKMARKS_KEY,
    queryFn: async (): Promise<Bookmark[]> => {
      const { data, error } = await client.GET("/api/bookmarks");
      if (error) {
        throw asError(error);
      }
      return data;
    },
  });
}

export async function createBookmark(bookmark: CreateBookmarkRequest): Promise<number> {
  const { data, error } = await client.POST("/api/bookmarks", {
    body: bookmark,
  });
  if (error) {
    throw asError(error);
  }
  return data.id;
}

export async function deleteBookmark(id: number): Promise<void> {
  const { error } = await client.DELETE("/api/bookmarks/{id}", {
    params: { path: { id } },
  });
  if (error) {
    throw asError(error);
  }
}

function asError(error: ApiError): Error {
  return new Error(error.detail ? `${error.error}: ${error.detail}` : error.error);
}

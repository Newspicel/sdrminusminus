// Typed REST access. `openapi-fetch` gives full inference from the generated schema; the thin
// query helpers key TanStack Query by path so WS `StateChanged` events can invalidate them
// (PLAN §4 step 4, §10). No polling — invalidation is WS-driven.
import { queryOptions } from "@tanstack/react-query";
import createClient from "openapi-fetch";
import type { paths } from "../generated/schema";
import type { ApiError, DeviceSettings, DevicesResponse, StateSnapshot } from "./types";

export const client = createClient<paths>({ baseUrl: "/" });

export const STATE_KEY = ["get", "/api/state"] as const;
export const DEVICES_KEY = ["get", "/api/devices"] as const;

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

function asError(error: ApiError): Error {
  return new Error(error.detail ? `${error.error}: ${error.detail}` : error.error);
}

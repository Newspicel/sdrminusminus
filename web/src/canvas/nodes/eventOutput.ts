import type { EventOutputTarget } from "../../lib/types";

export const OUTPUT_SERVICES = [
  { value: "webhook", label: "Webhook" },
  { value: "matrix", label: "Matrix" },
  { value: "mqtt", label: "MQTT" },
] as const;

export const WEBHOOK_FORMATS = [
  { value: "json", label: "JSON" },
  { value: "discord", label: "Discord" },
] as const;

export const SERVICE_LABELS: Record<EventOutputTarget["service"], string> = {
  webhook: "Webhook",
  matrix: "Matrix",
  mqtt: "MQTT",
};

export function newOutputTarget(service: EventOutputTarget["service"]): EventOutputTarget {
  switch (service) {
    case "webhook":
      return { service, url: "", format: "json" };
    case "matrix":
      return { service, homeserver_url: "", room_id: "", access_token: "" };
    case "mqtt":
      return { service, broker_url: "", topic: "", username: "", password: "" };
  }
}

export function eventOutputConfigured(target: EventOutputTarget): boolean {
  switch (target.service) {
    case "webhook":
      return target.url.trim() !== "";
    case "matrix":
      return (
        target.homeserver_url.trim() !== "" &&
        target.room_id.trim() !== "" &&
        target.access_token.trim() !== ""
      );
    case "mqtt":
      return target.broker_url.trim() !== "" && target.topic.trim() !== "";
  }
}

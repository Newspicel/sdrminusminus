import type { EventOutputTarget } from "../../lib/types";

export const OUTPUT_SERVICES = [
  { value: "beast", label: "ADS-B Beast TCP" },
  { value: "webhook", label: "Webhook" },
  { value: "matrix", label: "Matrix" },
  { value: "mqtt", label: "MQTT" },
  { value: "tunnel", label: "Network interface" },
] as const;

export const WEBHOOK_FORMATS = [
  { value: "json", label: "JSON" },
  { value: "discord", label: "Discord" },
] as const;

export const SERVICE_LABELS: Record<EventOutputTarget["service"], string> = {
  beast: "ADS-B Beast TCP",
  webhook: "Webhook",
  matrix: "Matrix",
  mqtt: "MQTT",
  tunnel: "Network interface",
};

export function newOutputTarget(service: EventOutputTarget["service"]): EventOutputTarget {
  switch (service) {
    case "beast":
      return { service, address: "127.0.0.1:30005", enabled: false };
    case "tunnel":
      return { service, interface: "", address: "10.23.0.1", prefix: 24 };
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
    case "beast":
      return target.enabled === true && target.address.trim() !== "";
    case "tunnel":
      return target.interface.trim() !== "";
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

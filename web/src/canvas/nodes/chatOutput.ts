import type { ChatOutputTarget } from "../../lib/types";

export const CHAT_SERVICES = [
  { value: "discord", label: "Discord" },
  { value: "matrix", label: "Matrix" },
] as const;

export function chatOutputConfigured(target: ChatOutputTarget): boolean {
  return target.service === "discord"
    ? target.webhook_url.trim() !== ""
    : target.homeserver_url.trim() !== "" &&
        target.room_id.trim() !== "" &&
        target.access_token.trim() !== "";
}

import { describe, expect, it } from "vitest";
import type { RemoteStatus } from "../lib/types";
import { appHost, remoteAction, remoteLine, remotePollMs } from "./remote";

function status(patch: Partial<RemoteStatus>): RemoteStatus {
  return { state: "unpaired", app_origin: "https://app.sdrmm.com", via_relay: false, ...patch };
}

describe("remote access", () => {
  it("polls only while something is about to change", () => {
    expect(remotePollMs(undefined)).toBe(false);
    expect(remotePollMs(status({ state: "pairing" }))).toBe(2000);
    expect(remotePollMs(status({ state: "connecting" }))).toBe(2000);
    expect(remotePollMs(status({ state: "retrying" }))).toBe(2000);
    expect(remotePollMs(status({ state: "online" }))).toBe(false);
    expect(remotePollMs(status({ state: "unpaired" }))).toBe(false);
  });

  it("offers the one action that fits the state", () => {
    expect(remoteAction(status({ state: "unpaired" }))).toBe("pair");
    expect(remoteAction(status({ state: "pairing" }))).toBe("cancel");
    expect(remoteAction(status({ state: "online" }))).toBe("disconnect");
    expect(remoteAction(status({ state: "retrying" }))).toBe("disconnect");
    expect(remoteAction(status({ state: "rejected" }))).toBe("pair-again");
  });

  it("offers nothing through the relay", () => {
    expect(remoteAction(status({ state: "online", via_relay: true }))).toBeNull();
    expect(remoteAction(status({ state: "unpaired", via_relay: true }))).toBeNull();
  });

  it("says why a connection is down", () => {
    expect(remoteLine(status({ state: "online" }))).toBe("Online");
    expect(remoteLine(status({ state: "retrying", error: "timed out" }))).toBe(
      "Offline: timed out",
    );
    expect(remoteLine(status({ state: "rejected", error: "device removed" }))).toBe(
      "Disconnected: device removed",
    );
    expect(remoteLine(status({ state: "unpaired" }))).toBe("");
  });

  it("names the app by host", () => {
    expect(appHost("https://app.sdrmm.com")).toBe("app.sdrmm.com");
    expect(appHost("http://localhost:5173")).toBe("localhost:5173");
    expect(appHost("not a url")).toBe("not a url");
  });
});

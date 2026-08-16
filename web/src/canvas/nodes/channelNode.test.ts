import { describe, expect, it } from "vitest";
import type { DeviceInfo, PatchGraph, PatchNode } from "../../lib/types";
import {
  channelBinding,
  channelBindingAction,
  channelBindingLabel,
  radioIsAttached,
  radioRefOf,
} from "./channelNode";

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

const graph: PatchGraph = {
  nodes: [
    node("dev", { kind: "device", data: { device: { backend: "rtlsdr", serial: "A" } } }),
    node("blank", { kind: "device", data: { device: null } }),
    node("nfm", { kind: "channel", data: { channel_type: "nfm" } }),
    node("loose", { kind: "channel", data: { channel_type: "am" } }),
  ],
  edges: [{ from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } }],
};

describe("radioRefOf", () => {
  it("names the radio the device node upstream is bound to", () => {
    expect(radioRefOf(graph, "nfm")).toEqual({ backend: "rtlsdr", serial: "A" });
    expect(radioRefOf(graph, "dev")).toEqual({ backend: "rtlsdr", serial: "A" });
    expect(radioRefOf(graph, "blank")).toBeNull();
    expect(radioRefOf(graph, "loose")).toBeNull();
  });
});

describe("radioIsAttached", () => {
  const attached: DeviceInfo[] = [{ driver: "rtlsdr", key: "0", label: "RTL-SDR", serial: "A" }];

  it("only matches a named radio that is on the bus", () => {
    expect(radioIsAttached({ backend: "rtlsdr", serial: "A" }, attached)).toBe(true);
    expect(radioIsAttached({ backend: "rtlsdr", serial: "B" }, attached)).toBe(false);
    expect(radioIsAttached({ backend: "rtlsdr", serial: "A" }, [])).toBe(false);
    expect(radioIsAttached(null, attached)).toBe(false);
  });
});

describe("channelBinding", () => {
  const state = { wired: true, open: false, named: true, attached: true };

  it("reads the wire before the radio", () => {
    expect(channelBinding({ ...state, wired: false, open: true })).toBe("unwired");
    expect(channelBinding({ ...state, open: true })).toBe("not-started");
    expect(channelBinding({ ...state, named: false })).toBe("no-radio");
    expect(channelBinding(state)).toBe("radio-closed");
    expect(channelBinding({ ...state, attached: false })).toBe("radio-absent");
  });

  it("offers an action only where one would do something", () => {
    expect(channelBindingAction("radio-closed")).toBe("Open radio");
    expect(channelBindingAction("not-started")).toBe("Start channel");
    expect(channelBindingAction("radio-absent")).toBeNull();
    expect(channelBindingAction("no-radio")).toBeNull();
    expect(channelBindingAction("unwired")).toBeNull();
  });

  it("labels each state for the node header", () => {
    expect(channelBindingLabel("radio-absent")).toBe("radio disconnected");
    expect(channelBindingLabel("not-started")).toBe("not started");
  });
});

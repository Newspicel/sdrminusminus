import { describe, expect, it } from "vitest";
import type { PatchGraph, RecordingInfo } from "../../lib/types";
import { MAX_NAME_LEN } from "../graph";
import {
  checkUpload,
  claimedRecordings,
  findRecording,
  recordingChoices,
  recordingDeviceId,
  recordingNodeFor,
  UPLOAD_ACCEPT,
} from "./recordingNode";

function recording(file: string, overrides: Partial<RecordingInfo> = {}): RecordingInfo {
  return {
    id: file.length,
    file,
    device_id: recordingDeviceId(file),
    device_label: "RTL-SDR 0",
    center_hz: 100e6,
    sample_rate: 2.048e6,
    samples: 2_048_000,
    bytes: 16_384_000,
    duration_s: 1,
    created_at: "2026-09-16T12:00:00Z",
    ...overrides,
  };
}

describe("recordingDeviceId", () => {
  it("names a recording by its stem alone", () => {
    expect(recordingDeviceId("2026-09-16T10-00-00_rtlsdr")).toBe(
      "recording:2026-09-16T10-00-00_rtlsdr",
    );
  });
});

describe("claimedRecordings", () => {
  const graph = {
    nodes: [
      { id: "a", kind: "recording", data: { recording: "airband" }, position: { x: 0, y: 0 } },
      { id: "b", kind: "recording", data: { recording: "weather" }, position: { x: 0, y: 0 } },
      { id: "c", kind: "recording", data: {}, position: { x: 0, y: 0 } },
      { id: "d", kind: "device", data: {}, position: { x: 0, y: 0 } },
    ],
    edges: [],
  } as unknown as PatchGraph;

  it("names what other recording nodes already play", () => {
    expect(claimedRecordings(graph, "a")).toEqual(["weather"]);
    expect(claimedRecordings(graph, "z")).toEqual(["airband", "weather"]);
  });
});

describe("recordingChoices", () => {
  const library = [
    recording("airband", { name: "Tower watch", tags: ["airband"] }),
    recording("weather", { note: "80 m net" }),
    recording("zulu"),
  ];

  it("offers what no other node holds, sorted by the name it shows", () => {
    expect(recordingChoices(library, ["weather"], "").map((rec) => rec.file)).toEqual([
      "airband",
      "zulu",
    ]);
  });

  it("searches the name, tags and note an operator wrote", () => {
    expect(recordingChoices(library, [], "tower").map((rec) => rec.file)).toEqual(["airband"]);
    expect(recordingChoices(library, [], "80 m").map((rec) => rec.file)).toEqual(["weather"]);
    expect(recordingChoices(library, [], "marine")).toEqual([]);
  });
});

describe("findRecording", () => {
  const library = [recording("airband")];

  it("finds a recording by stem and answers for one that is gone", () => {
    expect(findRecording(library, "airband")?.file).toBe("airband");
    expect(findRecording(library, "gone")).toBeNull();
    expect(findRecording(library, null)).toBeNull();
    expect(findRecording(library, "")).toBeNull();
  });
});

describe("checkUpload", () => {
  it("takes one archive, or one meta with one data", () => {
    expect(checkUpload(["take.sigmf"])).toBeNull();
    expect(checkUpload(["take.sigmf-meta", "take.sigmf-data"])).toBeNull();
    expect(checkUpload(["take.sigmf-data", "take.sigmf-meta"])).toBeNull();
  });

  it("says what is missing when only one half arrives", () => {
    expect(checkUpload([])).toBe("empty");
    expect(checkUpload(["take.sigmf-meta"])).toBe("lone-meta");
    expect(checkUpload(["take.sigmf-data"])).toBe("lone-data");
    expect(checkUpload(["a.sigmf-meta", "b.sigmf-meta"])).toBe("lone-meta");
  });

  it("refuses a pile of files that is not one recording", () => {
    expect(checkUpload(["a.sigmf", "b.sigmf"])).toBe("mixed");
    expect(checkUpload(["a.sigmf", "a.sigmf-meta"])).toBe("mixed");
  });

  it("offers the file picker every extension it takes", () => {
    expect(UPLOAD_ACCEPT.split(",")).toEqual([".sigmf", ".sigmf-meta", ".sigmf-data"]);
  });
});

describe("recordingNodeFor", () => {
  const at = { x: 40, y: 80 };

  it("opens a library row as a Recording node holding its stem", () => {
    expect(recordingNodeFor(recording("airband"), "recording:9f2c", at)).toEqual({
      id: "recording:9f2c",
      kind: "recording",
      data: { recording: "airband" },
      position: at,
      label: "airband",
    });
  });

  it("labels the node with the name an operator gave it", () => {
    const named = recording("airband", { name: "Tower watch" });
    expect(recordingNodeFor(named, "recording:9f2c", at).label).toBe("Tower watch");
  });

  it("keeps a long name inside the label a patch accepts", () => {
    const long = recording("airband", { name: "x".repeat(400) });
    expect(recordingNodeFor(long, "recording:9f2c", at).label).toHaveLength(MAX_NAME_LEN);
  });
});

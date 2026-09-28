export type PortType = "iq" | "audio" | "events";

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Point {
  x: number;
  y: number;
}

interface Column {
  x: number;
  w: number;
}

const GRID = {
  wide: { widths: [150, 150, 140], gap: 60 },
  narrow: { widths: [100, 100, 100], gap: 22 },
};

const MARGIN = 8;
const TOP = 24;
const HEIGHT = 340;

function columns(widths: number[], gap: number): Column[] {
  let x = 0;
  return widths.map((w) => {
    const column = { x, w };
    x += w + gap;
    return column;
  });
}

function place(column: Column | undefined, y: number, h: number): Box {
  return { x: column?.x ?? 0, y, w: column?.w ?? 0, h };
}

export function route(from: Point, to: Point, bend = to.x): string {
  if (from.y === to.y) {
    return `M${from.x} ${from.y} H${to.x}`;
  }
  const pull = (bend - from.x) / 2;
  const curve = `M${from.x} ${from.y} C${from.x + pull} ${from.y} ${bend - pull} ${to.y} ${bend} ${to.y}`;
  return bend < to.x ? `${curve} H${to.x}` : curve;
}

const input = (box: Box): Point => ({ x: box.x, y: box.y + 22 });
const output = (box: Box, index = 0): Point => ({ x: box.x + box.w, y: box.y + 22 + index * 24 });

export function patchLayout(narrow: boolean) {
  const grid = narrow ? GRID.narrow : GRID.wide;
  const [source, channel, sink] = columns(grid.widths, grid.gap);
  const box = {
    radio: place(source, 150, 78),
    channel: place(channel, 150, 78),
    speaker: place(sink, 40, 70),
    log: place(sink, 174, 78),
    recorder: place(sink, 280, 70),
  };

  const iq = output(box.radio);
  const audio = output(box.channel);
  const events = output(box.channel, 1);

  const wires = [
    { id: "tune", port: "iq", d: route(iq, input(box.channel)) },
    { id: "listen", port: "audio", d: route(audio, input(box.speaker)) },
    { id: "decode", port: "events", d: route(events, input(box.log)) },
    { id: "record", port: "iq", d: route(iq, input(box.recorder), box.channel.x + grid.gap) },
  ];

  const ports: (Point & { type: PortType })[] = [
    { type: "iq", ...iq },
    { type: "iq", ...input(box.channel) },
    { type: "audio", ...audio },
    { type: "events", ...events },
    { type: "audio", ...input(box.speaker) },
    { type: "events", ...input(box.log) },
    { type: "iq", ...input(box.recorder) },
  ];

  const nodes = [
    { ...box.radio, title: "RTL-SDR", cat: "source" },
    { ...box.channel, title: "Broadcast FM", cat: "channel" },
    { ...box.speaker, title: "Speaker", cat: "output" },
    { ...box.log, title: "Decoder log", cat: "output" },
    { ...box.recorder, title: "Recorder", cat: "output" },
  ];

  const width = box.recorder.x + box.recorder.w + 2 * MARGIN;
  const viewBox = `${-MARGIN} ${TOP} ${width} ${HEIGHT}`;

  return { viewBox, width, box, nodes, ports, wires };
}

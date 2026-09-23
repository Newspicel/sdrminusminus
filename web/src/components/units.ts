export interface UnitInfo {
  name: string;
  about: string;
}

const PREFIXES: ReadonlyArray<readonly [string, string, string]> = [
  ["G", "Giga", "1,000,000,000"],
  ["M", "Mega", "1,000,000"],
  ["k", "Kilo", "1,000"],
  ["m", "Milli", "0.001"],
  ["µ", "Micro", "0.000001"],
];

const SCALABLE: Readonly<Record<string, UnitInfo>> = {
  Hz: { name: "hertz", about: "Cycles per second." },
  "S/s": { name: "samples per second", about: "How often the signal is measured." },
  "bit/s": { name: "bits per second", about: "Data rate." },
  Bd: { name: "baud", about: "Symbols sent per second." },
  B: { name: "bytes", about: "Data size, 8 bits each." },
  s: { name: "seconds", about: "Time." },
  m: { name: "meters", about: "Distance." },
  W: { name: "watts", about: "Power." },
};

const FIXED: Readonly<Record<string, UnitInfo>> = {
  dB: { name: "Decibel", about: "Ratio on a log scale. +3 dB doubles power, +10 dB is ten times." },
  dBFS: {
    name: "Decibel full scale",
    about: "Level relative to the loudest the receiver can capture. 0 is the top.",
  },
  "dBFS/Hz": { name: "Decibel full scale per hertz", about: "Level in each hertz of bandwidth." },
  "dB/Hz": { name: "Decibel per hertz", about: "Level in each hertz of bandwidth." },
  dBm: { name: "Decibel milliwatt", about: "Absolute power. 0 dBm is 1 mW, −30 dBm is 1 µW." },
  dBc: { name: "Decibel relative to carrier", about: "Level compared to the main signal." },
  dBi: { name: "Decibel isotropic", about: "Antenna gain over one radiating equally everywhere." },
  "dB SNR": { name: "Signal to noise ratio", about: "How far the signal stands above the noise." },
  ppm: { name: "Parts per million", about: "Clock error. 1 ppm at 100 MHz is 100 Hz off." },
  "°": { name: "Degrees", about: "Angle, 360 in a full turn." },
};

function scaled(symbol: string): UnitInfo | undefined {
  for (const [prefix, word, amount] of PREFIXES) {
    const base = symbol.startsWith(prefix) ? SCALABLE[symbol.slice(prefix.length)] : undefined;
    if (base !== undefined) {
      return { name: `${word}${base.name}`, about: `${amount} ${base.name}. ${base.about}` };
    }
  }
  return undefined;
}

function plain(symbol: string): UnitInfo | undefined {
  const base = SCALABLE[symbol];
  if (base === undefined) {
    return undefined;
  }
  return { name: base.name.charAt(0).toUpperCase() + base.name.slice(1), about: base.about };
}

export function unitInfo(symbol: string): UnitInfo | undefined {
  return FIXED[symbol] ?? plain(symbol) ?? scaled(symbol);
}

export function unitTip(symbol: string): string | undefined {
  const info = unitInfo(symbol);
  return info === undefined ? undefined : `${info.name}. ${info.about}`;
}

export function splitUnit(text: string): readonly [string, string] | undefined {
  const match = /^(.*\d)\s(\S+(?: SNR)?)$/u.exec(text);
  const value = match?.[1];
  const symbol = match?.[2];
  if (value === undefined || symbol === undefined || unitInfo(symbol) === undefined) {
    return undefined;
  }
  return [value, symbol];
}

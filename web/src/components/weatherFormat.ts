import type { AprsWeather, AptImage, LrptImage, SondeType, WefaxPicture } from "../lib/types";

type AvhrrChannel = NonNullable<AptImage["channel_a"]>;

export const LRPT_MODE_LABELS: Record<LrptImage["mode"], string> = {
  qpsk72: "QPSK 72k",
  oqpsk72: "OQPSK 72k",
  oqpsk80: "OQPSK 80k",
};

export const WEFAX_IOC_VALUES: Record<WefaxPicture["ioc"], number> = {
  ioc576: 576,
  ioc288: 288,
};

export const WEFAX_LPM_VALUES: Record<WefaxPicture["lpm"], number> = {
  lpm60: 60,
  lpm90: 90,
  lpm120: 120,
  lpm240: 240,
};

export const SONDE_LABELS: Record<SondeType, string> = {
  rs41: "RS41",
  dfm: "DFM",
  m10: "M10",
  m20: "M20",
  imet4: "iMet-4",
};

export const AVHRR_LABELS: Record<AvhrrChannel, string> = {
  ch1: "1",
  ch2: "2",
  ch3a: "3A",
  ch3b: "3B",
  ch4: "4",
  ch5: "5",
};

export function avhrrChannels(image: Pick<AptImage, "channel_a" | "channel_b">): string | null {
  const a = image.channel_a == null ? "?" : AVHRR_LABELS[image.channel_a];
  const b = image.channel_b == null ? "?" : AVHRR_LABELS[image.channel_b];
  return image.channel_a == null && image.channel_b == null ? null : `ch ${a} + ${b}`;
}

type Maybe = number | null | undefined;

function unit(value: Maybe, digits: number, symbol: string): string | undefined {
  return value == null ? undefined : `${value.toFixed(digits)} ${symbol}`;
}

export function celsius(value: Maybe): string | undefined {
  return unit(value, 1, "°C");
}

export function percent(value: Maybe): string | undefined {
  return value == null ? undefined : `${Math.round(value)}%`;
}

export function hectopascal(value: Maybe): string | undefined {
  return unit(value, 1, "hPa");
}

export function metresPerSecond(value: Maybe): string | undefined {
  return unit(value, 1, "m/s");
}

export function climbRate(value: Maybe): string | undefined {
  return value == null ? undefined : `${value >= 0 ? "+" : ""}${value.toFixed(1)} m/s`;
}

export function metres(value: Maybe): string | undefined {
  return value == null ? undefined : `${Math.round(value).toLocaleString("en-US")} m`;
}

export function millimetres(value: Maybe): string | undefined {
  return unit(value, 1, "mm");
}

export function bearing(value: Maybe): string | undefined {
  return value == null ? undefined : `${Math.round(value)}°`;
}

export function seconds(ms: number): string {
  return `${(ms / 1000).toFixed(1)} s`;
}

export function wind(weather: AprsWeather): string | undefined {
  const speed = metresPerSecond(weather.wind_speed_ms);
  if (speed === undefined) {
    return undefined;
  }
  const from = bearing(weather.wind_dir_deg);
  return from === undefined ? speed : `${speed} from ${from}`;
}

export function aprsWeatherRows(weather: AprsWeather): [string, string | undefined][] {
  return [
    ["Wind", wind(weather)],
    ["Gust", metresPerSecond(weather.wind_gust_ms)],
    ["Temperature", celsius(weather.temperature_c)],
    ["Humidity", percent(weather.humidity_pct)],
    ["Pressure", hectopascal(weather.pressure_hpa)],
    ["Rain 1 h", millimetres(weather.rain_1h_mm)],
    ["Rain 24 h", millimetres(weather.rain_24h_mm)],
    ["Rain since midnight", millimetres(weather.rain_midnight_mm)],
    ["Snow 24 h", millimetres(weather.snow_24h_mm)],
    ["Luminosity", weather.luminosity_wm2 == null ? undefined : `${weather.luminosity_wm2} W/m²`],
  ];
}

export function aprsWeatherFact(weather: AprsWeather): string {
  return [
    celsius(weather.temperature_c),
    weather.humidity_pct == null ? undefined : `${weather.humidity_pct}%`,
    hectopascal(weather.pressure_hpa),
    windFact(weather),
    weather.rain_1h_mm == null ? undefined : `${weather.rain_1h_mm.toFixed(1)} mm/h`,
  ]
    .filter((part) => part !== undefined)
    .join(" · ");
}

function windFact(weather: AprsWeather): string | undefined {
  const speed = metresPerSecond(weather.wind_speed_ms);
  if (speed === undefined || weather.wind_dir_deg == null) {
    return speed;
  }
  return `${String(weather.wind_dir_deg).padStart(3, "0")}° ${speed}`;
}

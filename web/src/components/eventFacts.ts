import type { DecoderEvent } from "../lib/types";
import { candidateScore, dvMode, dvNetwork, dvParties, modulationLabel } from "./decoderViews";
import { SSTV_MODE_LABELS } from "./sstvModes";

export function hex5(address: number): string {
  return address.toString(16).toUpperCase().padStart(5, "0");
}

export function hex2(value: number): string {
  return value.toString(16).toUpperCase().padStart(2, "0");
}

export type SensorReading = {
  model: string;
  id: number;
  channel?: number | null;
  temperature_c?: number | null;
  humidity_pct?: number | null;
  moisture_pct?: number | null;
  pressure_kpa?: number | null;
  wind_avg_kmh?: number | null;
  wind_max_kmh?: number | null;
  wind_dir_deg?: number | null;
  rain_mm?: number | null;
  power_w?: number | null;
  energy_kwh?: number | null;
};

export function sensorFacts(reading: SensorReading): (string | null)[] {
  return [
    reading.model,
    `id ${hex2(reading.id)}`,
    reading.channel == null ? null : `ch ${reading.channel}`,
    reading.pressure_kpa == null ? null : `${reading.pressure_kpa.toFixed(0)} kPa`,
    reading.temperature_c == null ? null : `${reading.temperature_c.toFixed(1)} °C`,
    reading.humidity_pct == null ? null : `${reading.humidity_pct.toFixed(0)} %`,
    reading.moisture_pct == null ? null : `soil ${reading.moisture_pct.toFixed(0)} %`,
    reading.wind_avg_kmh == null ? null : `wind ${reading.wind_avg_kmh.toFixed(1)} km/h`,
    reading.wind_dir_deg == null ? null : `from ${reading.wind_dir_deg.toFixed(0)}°`,
    reading.rain_mm == null ? null : `rain ${reading.rain_mm.toFixed(1)} mm`,
    reading.power_w == null ? null : `${reading.power_w.toFixed(0)} W`,
  ];
}

function position(lat: number | null | undefined, lon: number | null | undefined): string | null {
  return lat == null || lon == null ? null : `${lat.toFixed(4)}, ${lon.toFixed(4)}`;
}

export function hasPosition(event: DecoderEvent): boolean {
  const data = event.data as { lat?: number | null; lon?: number | null } | undefined;
  return data?.lat != null && data?.lon != null;
}

function broadcastSystem(system: string): string {
  const labels: Record<string, string> = {
    dab: "DAB",
    dab_plus: "DAB+",
    dvb_s: "DVB-S",
    dvb_s2: "DVB-S2",
    drm30: "DRM30",
    drm_plus: "DRM+",
  };
  return labels[system] ?? system;
}

function join(parts: readonly (string | null)[]): string {
  return parts.filter((p) => p !== null).join(" · ");
}

type EventData<K extends DecoderEvent["kind"]> = Extract<DecoderEvent, { kind: K }>["data"];

function subghzSummary(f: EventData<"subghz">): string {
  return join([
    ...(f.reading == null
      ? [f.bits === 0 ? `raw, ${(f.timings_us ?? []).length} edges` : `${f.bits} bit ${f.data}`]
      : sensorFacts(f.reading)),
    f.address == null ? null : `addr ${hex5(f.address)}`,
    f.button == null ? null : `btn ${f.button.toString(16).toUpperCase()}`,
    f.repeats > 1 ? `\u00d7${f.repeats}` : null,
  ]);
}

function callSummary(c: EventData<"call">): string {
  return join([
    c.mode.toUpperCase(),
    c.destination == null
      ? null
      : c.group_call === false
        ? `radio ${c.destination}`
        : `talkgroup ${c.destination}`,
    c.source == null ? null : `from ${c.source}`,
    c.slot == null ? null : `TS${c.slot}`,
    `${(c.duration_ms / 1000).toFixed(1)} s`,
    c.emergency ? "emergency" : null,
    c.encrypted ? "encrypted" : null,
  ]);
}

function dvSummary(f: EventData<"dv">): string {
  return join([
    dvMode(f),
    dvNetwork(f) || null,
    dvParties(f) || null,
    f.via == null ? null : `via ${f.via}`,
    f.opcode ?? null,
    f.encrypted === true ? "encrypted" : null,
    f.text ?? null,
  ]);
}

function identSummary(r: EventData<"ident">): string {
  const best = r.candidates?.[0];
  return join([
    modulationLabel(r),
    r.modulation === "none" ? null : `${(r.bandwidth_hz / 1000).toFixed(1)} kHz`,
    r.symbol_rate_hz == null ? null : `${Math.round(r.symbol_rate_hz)} Bd`,
    r.deviation_hz == null ? null : `\u00b1${Math.round(r.deviation_hz)} Hz`,
    best == null ? null : `${best.name} (${candidateScore(best)})`,
  ]);
}

function broadcastSummary(status: EventData<"broadcast">): string {
  return join([
    broadcastSystem(status.system),
    status.locked ? "locked" : "searching",
    status.locked ? `${status.snr_db.toFixed(1)} dB SNR` : null,
    status.locked
      ? `${status.frequency_error_hz >= 0 ? "+" : ""}${status.frequency_error_hz.toFixed(0)} Hz`
      : null,
    status.label ?? null,
  ]);
}

export function eventSummary(event: DecoderEvent): string {
  switch (event.kind) {
    case "rds": {
      const r = event.data;
      return join([
        r.pi == null ? null : `PI ${r.pi}`,
        r.ps ?? null,
        r.pty_name ?? null,
        r.radiotext ?? null,
      ]);
    }
    case "pocsag": {
      const p = event.data;
      return p.text === "" ? `${p.address} (${p.function})` : `${p.address}: ${p.text}`;
    }
    case "flex": {
      const p = event.data;
      return p.text === "" ? `${p.address} · ${p.payload}` : `${p.address}: ${p.text}`;
    }
    case "ermes": {
      const p = event.data;
      return p.text === "" ? `${p.local_address} · ${p.payload}` : `${p.local_address}: ${p.text}`;
    }
    case "adsb": {
      const a = event.data;
      return join([
        a.icao,
        a.callsign?.trim() ?? null,
        a.altitude_ft == null ? null : `${a.altitude_ft} ft`,
        position(a.lat, a.lon),
      ]);
    }
    case "ais": {
      const m = event.data;
      return join([String(m.mmsi), m.name?.trim() ?? null, position(m.lat, m.lon)]);
    }
    case "aprs": {
      const p = event.data;
      return p.mic_e_message == null ? p.tnc2 : join([p.tnc2, p.mic_e_message]);
    }
    case "rtty":
    case "morse":
      return event.data.text;
    case "psk":
      return join([event.data.baud.toUpperCase(), event.data.text]);
    case "cw_skimmer": {
      const spot = event.data;
      return join([
        `${spot.offset_hz >= 0 ? "+" : ""}${spot.offset_hz.toFixed(0)} Hz`,
        `${spot.wpm.toFixed(0)} WPM`,
        spot.text,
      ]);
    }
    case "ft8":
    case "ft4": {
      const message = event.data;
      return join([
        message.text,
        `${message.snr_db >= 0 ? "+" : ""}${message.snr_db.toFixed(0)} dB`,
        `${message.audio_hz.toFixed(0)} Hz`,
      ]);
    }
    case "wspr": {
      const spot = event.data;
      return join([
        spot.text,
        `${spot.snr_db >= 0 ? "+" : ""}${spot.snr_db.toFixed(0)} dB`,
        `${spot.audio_hz.toFixed(0)} Hz`,
      ]);
    }
    case "selcall":
      return `${event.data.system === "ccir1" ? "CCIR-1" : "ZVEI-1"} · ${event.data.code}`;
    case "navtex": {
      const n = event.data;
      const header =
        n.station == null || n.subject == null || n.serial == null
          ? null
          : `${n.station}${n.subject}${String(n.serial).padStart(2, "0")}`;
      return join([header, n.subject_name ?? null, n.text.replaceAll("\n", " ").trim()]);
    }
    case "acars": {
      const a = event.data;
      const text = a.text.replaceAll("\n", " ").trim();
      return join([a.registration, a.flight?.trim() ?? null, `[${a.label}]`, text || null]);
    }
    case "subghz":
      return subghzSummary(event.data);
    case "scrambler": {
      const s = event.data;
      return s.inversion_hz == null
        ? "no inversion"
        : `inversion ${s.inversion_hz.toFixed(0)} Hz · ${(s.confidence * 100).toFixed(0)}% confidence`;
    }
    case "tone": {
      const t = event.data;
      const heard = join([
        t.ctcss_hz == null ? null : `CTCSS ${t.ctcss_hz.toFixed(1)} Hz`,
        t.dcs_code == null ? null : `DCS ${String(t.dcs_code).padStart(3, "0")}`,
      ]);
      return join([heard === "" ? "no tone" : heard, t.open ? "open" : "muted"]);
    }
    case "call":
      return callSummary(event.data);
    case "dv":
      return dvSummary(event.data);
    case "ident":
      return identSummary(event.data);
    case "broadcast":
      return broadcastSummary(event.data);
    case "radio_clock": {
      const r = event.data;
      return join([r.standard.toUpperCase(), r.datetime, r.leap_warning ? "leap warning" : null]);
    }
    case "gnss": {
      const g = event.data;
      return join([
        `GPS PRN ${g.prn}`,
        `${g.doppler_hz >= 0 ? "+" : ""}${g.doppler_hz.toFixed(0)} Hz`,
        `${g.cn0_db_hz.toFixed(1)} dB-Hz`,
        g.subframe == null ? "acquired" : `subframe ${g.subframe}`,
        g.tow_seconds == null ? null : `TOW ${g.tow_seconds} s`,
      ]);
    }
    case "sstv": {
      const p = event.data;
      return join([
        SSTV_MODE_LABELS[p.mode],
        `${p.width}\u00d7${p.height}`,
        p.complete
          ? `complete in ${Math.floor(p.duration_ms / 1000)} s`
          : `${p.lines} of ${p.height} lines`,
      ]);
    }
    case "vor": {
      const reading = event.data;
      return join([
        reading.station ?? "VOR",
        `${reading.radial_deg.toFixed(1)}° radial`,
        `${Math.round(reading.confidence * 100)}%`,
      ]);
    }
    case "ils": {
      const reading = event.data;
      return join([
        reading.component === "localizer" ? "localizer" : "glideslope",
        `${reading.ddm >= 0 ? "+" : ""}${reading.ddm.toFixed(3)} DDM`,
        `${reading.deviation_dots >= 0 ? "+" : ""}${reading.deviation_dots.toFixed(2)} dots`,
      ]);
    }
    case "dsc":
    case "inmarsat_stdc":
    case "inmarsat_aero":
    case "vdl2":
    case "hfdl":
    case "iridium": {
      const message = event.data;
      return join([
        message.message_type,
        message.station ?? null,
        message.text?.replaceAll("\n", " ").trim() ?? null,
      ]);
    }
  }
}

export function eventStation(event: DecoderEvent): string | null {
  switch (event.kind) {
    case "adsb":
      return event.data.icao;
    case "ais":
      return String(event.data.mmsi);
    case "aprs":
      return event.data.source;
    case "pocsag":
      return String(event.data.address);
    case "flex":
      return String(event.data.address);
    case "ermes":
      return String(event.data.local_address);
    case "rds":
      return event.data.pi ?? null;
    case "navtex":
      return event.data.station ?? null;
    case "acars":
      return event.data.registration;
    case "subghz": {
      const f = event.data;
      if (f.reading != null) {
        return `${f.reading.model} ${hex2(f.reading.id)}`;
      }
      if (f.address != null) {
        return hex5(f.address);
      }
      return f.data === "" ? null : f.data;
    }
    case "dv":
      return (
        event.data.source_call ?? (event.data.source == null ? null : String(event.data.source))
      );
    case "call":
      return event.data.source == null ? null : String(event.data.source);
    case "ft8":
    case "ft4":
      return event.data.text.split(/\s+/)[1] ?? null;
    case "wspr":
      return event.data.callsign;
    case "radio_clock":
      return event.data.standard.toUpperCase();
    case "gnss":
      return `GPS-${event.data.prn}`;
    case "vor":
      return event.data.station ?? null;
    case "dsc":
    case "inmarsat_stdc":
    case "inmarsat_aero":
    case "vdl2":
    case "hfdl":
    case "iridium":
      return event.data.station ?? null;
    case "rtty":
    case "morse":
    case "cw_skimmer":
    case "psk":
    case "selcall":
    case "tone":
    case "scrambler":
    case "ident":
    case "ils":
      return null;
    case "sstv":
      return SSTV_MODE_LABELS[event.data.mode];
    case "broadcast": {
      const status = event.data;
      const id = status.service_id ?? status.ensemble_id;
      return id == null ? null : id.toString(16).toUpperCase();
    }
  }
}

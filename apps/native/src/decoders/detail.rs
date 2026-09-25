use sdrmm_wire::{
    Transmission,
    channel::{IlsComponent, SelcallSystem},
    decode::{
        AcarsMessage, AdsbMessage, AisMessage, AprsPacket, BroadcastData, BroadcastStatus,
        DataLinkMessage, DecoderEvent, DectFrame, DectSide, DvFrame, DvFrameKind, ErmesMessage,
        FlexMessage, GnssFrame, IdentReport, NavtexMessage, PocsagMessage, RadioClockFrame,
        RdsUpdate, SubghzFrame, SubghzReading, VorReading, WsjtMessage, WsprSpot,
    },
    rest::VoiceCall,
    units::hertz,
};

use crate::decoders::{
    text::{bare_hex, degrees, feet, flag, hex, knots, percent, position, serde_name, signed},
    views::{
        candidate_score, dv_checksum, dv_network, dv_trunking, ident_measurements, ident_overview,
        modulation_label, signal_frequency,
    },
};

pub type Field = (&'static str, String);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventDetail {
    pub fields: Vec<Field>,
    pub body: Option<String>,
}

type Row = (&'static str, Option<String>);

fn detail(rows: impl IntoIterator<Item = Row>, body: Option<String>) -> EventDetail {
    EventDetail {
        fields: rows
            .into_iter()
            .filter_map(|(label, value)| value.filter(|v| !v.is_empty()).map(|v| (label, v)))
            .collect(),
        body: body.filter(|text| !text.is_empty()),
    }
}

fn some(text: impl Into<String>) -> Option<String> {
    Some(text.into())
}

fn trimmed(text: Option<&String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
}

fn repaired(count: u32, unit: &str) -> Option<String> {
    (count > 0).then(|| format!("{count}{unit}"))
}

#[must_use]
pub fn event_detail(event: &DecoderEvent) -> EventDetail {
    match event {
        DecoderEvent::Transmission(t) => transmission(t),
        DecoderEvent::Rds(r) => rds(r),
        DecoderEvent::Pocsag(p) => pocsag(p),
        DecoderEvent::Flex(p) => flex(p),
        DecoderEvent::Ermes(p) => ermes(p),
        DecoderEvent::Adsb(a) => adsb(a),
        DecoderEvent::Ais(m) => ais(m),
        DecoderEvent::Aprs(p) => aprs(p),
        DecoderEvent::Rtty(t) => detail([], some(&t.text)),
        DecoderEvent::Morse(m) => detail(
            [("Speed", some(format!("{:.0} WPM", m.wpm)))],
            some(&m.text),
        ),
        DecoderEvent::CwSkimmer(m) => detail(
            [
                (
                    "Offset",
                    some(format!("{} Hz", signed(f64::from(m.offset_hz), 0))),
                ),
                ("Speed", some(format!("{:.0} WPM", m.wpm))),
                (
                    "SNR",
                    some(format!("{} dB", signed(f64::from(m.snr_db), 1))),
                ),
            ],
            some(&m.text),
        ),
        DecoderEvent::Ft8(m) | DecoderEvent::Ft4(m) => wsjt(m),
        DecoderEvent::Psk(t) => detail(
            [("Mode", some(serde_name(&t.baud).to_uppercase()))],
            some(&t.text),
        ),
        DecoderEvent::Wspr(s) => wspr(s),
        DecoderEvent::Selcall(s) => detail(
            [
                ("Tone plan", some(selcall_plan(s.system))),
                ("Code", some(&s.code)),
                ("Tone duration", some(format!("{} ms", s.tone_ms))),
            ],
            None,
        ),
        DecoderEvent::Navtex(n) => navtex(n),
        DecoderEvent::Acars(a) => acars(a),
        DecoderEvent::Subghz(f) => subghz(f),
        DecoderEvent::Ident(r) => ident(r),
        DecoderEvent::Scrambler(s) => detail(
            [
                ("Inversion", s.inversion_hz.map(|hz| format!("{hz:.0} Hz"))),
                (
                    "Confidence",
                    s.inversion_hz
                        .map(|_| format!("{:.0}%", s.confidence * 100.0)),
                ),
            ],
            None,
        ),
        DecoderEvent::Tone(t) => detail(
            [
                ("CTCSS", t.ctcss_hz.map(|hz| format!("{hz:.1} Hz"))),
                ("DCS", t.dcs_code.map(|code| format!("{code:03}"))),
                ("Audio", some(if t.open { "open" } else { "muted" })),
            ],
            None,
        ),
        DecoderEvent::Call(c) => call(c),
        DecoderEvent::Dv(f) => dv(f),
        DecoderEvent::BroadcastData(data) => broadcast_data(data),
        DecoderEvent::Broadcast(status) => broadcast(status),
        DecoderEvent::RadioClock(r) => radio_clock(r),
        DecoderEvent::Gnss(g) => gnss(g),
        DecoderEvent::Sstv(p) => detail(
            [
                ("Mode", some(p.mode.label())),
                ("Size", some(format!("{} × {}", p.width, p.height))),
                (
                    "Lines received",
                    some(format!("{} of {}", p.lines, p.height)),
                ),
                (
                    "State",
                    some(if p.complete { "complete" } else { "cut short" }),
                ),
                (
                    "Took",
                    some(format!("{:.1} s", f64::from(p.duration_ms) / 1000.0)),
                ),
            ],
            None,
        ),
        DecoderEvent::Vor(v) => vor(v),
        DecoderEvent::Df(b) => detail(
            [
                ("Station", b.station_id.clone()),
                ("Bearing", some(format!("{:.2}°", b.bearing_deg))),
                ("Confidence", some(percent(f64::from(b.confidence)))),
                ("Seen from", position(b.lat, b.lon)),
            ],
            None,
        ),
        DecoderEvent::DfFix(e) => detail(
            [
                ("Position", position(Some(e.lat), Some(e.lon))),
                (
                    "Uncertainty",
                    some(format!(
                        "{} × {} m",
                        e.ellipse_major_m.round(),
                        e.ellipse_minor_m.round()
                    )),
                ),
                (
                    "Ellipse bearing",
                    some(format!("{:.1}°", e.ellipse_bearing_deg)),
                ),
                ("Bearings used", some(e.samples.to_string())),
            ],
            None,
        ),
        DecoderEvent::Radar(d) => detail(
            [
                ("Range bin", some(d.range_bin.to_string())),
                ("Bistatic range", some(format!("{:.2} km", d.range_km))),
                (
                    "Doppler",
                    some(format!("{} Hz", signed(f64::from(d.doppler_hz), 1))),
                ),
                ("SNR", some(format!("{:.1} dB", d.snr_db))),
            ],
            None,
        ),
        DecoderEvent::Ils(i) => detail(
            [
                (
                    "Component",
                    some(match i.component {
                        IlsComponent::Localizer => "localizer",
                        IlsComponent::Glideslope => "glideslope",
                    }),
                ),
                (
                    "90 Hz modulation",
                    some(format!("{:.2}%", i.modulation_90 * 100.0)),
                ),
                (
                    "150 Hz modulation",
                    some(format!("{:.2}%", i.modulation_150 * 100.0)),
                ),
                ("DDM", some(signed(f64::from(i.ddm), 4))),
                (
                    "Deviation",
                    some(format!("{} dots", signed(f64::from(i.deviation_dots), 2))),
                ),
                ("Signal", some(format!("{:.1} dB", i.signal_db))),
            ],
            None,
        ),
        DecoderEvent::Dsc(m)
        | DecoderEvent::InmarsatStdc(m)
        | DecoderEvent::InmarsatAero(m)
        | DecoderEvent::Vdl2(m)
        | DecoderEvent::Hfdl(m)
        | DecoderEvent::Iridium(m) => data_link(m),
        DecoderEvent::Dect(frame) => dect(frame),
    }
}

fn transmission(t: &Transmission) -> EventDetail {
    let decoder = t.decoder.as_ref().map(|decoder| {
        let how = if t.decoder_confirmed {
            "confirmed"
        } else {
            "estimated"
        };
        format!("{} ({how})", decoder.to_uppercase())
    });
    detail(
        [
            ("Transmission", some(t.id.to_string())),
            ("State", some(serde_name(&t.state))),
            ("Frequency", some(hertz(t.signal.frequency_hz))),
            ("Bandwidth", some(hertz(t.signal.bandwidth_hz))),
            (
                "Modulation",
                some(modulation_label(t.signal.modulation, t.signal.sideband)),
            ),
            ("Decoder", decoder),
            ("Confidence", some(percent(f64::from(t.signal.confidence)))),
            ("SNR", some(format!("{:.1} dB", t.signal.snr_db))),
            (
                "Duration",
                some(format!("{:.2} s", t.duration_ms as f64 / 1000.0)),
            ),
            ("Error", t.error.clone()),
        ],
        None,
    )
}

fn rds(r: &RdsUpdate) -> EventDetail {
    let programme = r
        .pty_name
        .clone()
        .or_else(|| r.pty.map(|code| format!("PTY {code}")));
    let content = r
        .music
        .map(|music| if music { "music" } else { "speech" }.to_owned());
    let alternatives: Vec<String> = r.alt_freqs_hz.iter().copied().map(hertz).collect();
    detail(
        [
            ("PI", r.pi.clone()),
            ("Station", trimmed(r.ps.as_ref())),
            ("Programme type", programme),
            ("Traffic programme", flag(r.tp)),
            ("Traffic announcement", flag(r.ta)),
            ("Content", content),
            ("Alternative frequencies", some(alternatives.join(", "))),
            ("Groups", some(r.groups.to_string())),
            ("Block errors", some(r.block_errors.to_string())),
        ],
        trimmed(r.radiotext.as_ref()),
    )
}

fn pocsag(p: &PocsagMessage) -> EventDetail {
    let letter = "ABCD"
        .chars()
        .nth(usize::from(p.function))
        .map_or_else(|| p.function.to_string(), String::from);
    detail(
        [
            ("RIC", some(format!("{:07}", p.address))),
            ("Function", some(format!("{letter} ({})", p.function))),
            ("Baud", some(p.baud.to_string())),
            ("Payload", some(serde_name(&p.payload))),
            ("Repaired", repaired(p.errors_corrected, "")),
        ],
        some(&p.text),
    )
}

fn flex(p: &FlexMessage) -> EventDetail {
    detail(
        [
            ("Address", some(p.address.to_string())),
            ("Payload", some(serde_name(&p.payload))),
            ("Mode", some(format!("{}/{}", p.baud, p.levels))),
            ("Cycle", some(p.cycle.to_string())),
            ("Frame", some(p.frame.to_string())),
            ("Phase", some(p.phase.to_string())),
            ("Repaired", repaired(p.errors_corrected, "")),
        ],
        some(&p.text),
    )
}

fn ermes(p: &ErmesMessage) -> EventDetail {
    detail(
        [
            ("Local address", some(p.local_address.to_string())),
            ("Message number", some(p.message_number.to_string())),
            ("Payload", some(serde_name(&p.payload))),
            ("Urgent", flag(Some(p.urgent))),
            ("Alert", some(p.alert.to_string())),
            ("Repaired", repaired(p.errors_corrected, "")),
        ],
        some(&p.text),
    )
}

fn adsb(a: &AdsbMessage) -> EventDetail {
    let altitude = if a.on_ground == Some(true) {
        some("on ground")
    } else {
        feet(a.altitude_ft)
    };
    detail(
        [
            ("ICAO", some(a.icao.to_uppercase())),
            ("Callsign", trimmed(a.callsign.as_ref())),
            ("Downlink format", some(a.df.to_string())),
            ("Type code", a.type_code.map(|code| code.to_string())),
            ("Altitude", altitude),
            ("Position", position(a.lat, a.lon)),
            ("Ground speed", knots(a.ground_speed_kt)),
            ("Track", degrees(a.track_deg)),
            (
                "Vertical rate",
                a.vertical_rate_fpm.map(|fpm| format!("{fpm} ft/min")),
            ),
            ("Squawk", a.squawk.clone()),
            ("Raw", some(&a.raw)),
        ],
        None,
    )
}

fn ais(m: &AisMessage) -> EventDetail {
    detail(
        [
            ("MMSI", some(m.mmsi.to_string())),
            ("Message type", some(m.msg_type.to_string())),
            ("Channel", some(m.ais_channel.to_string())),
            ("Name", trimmed(m.name.as_ref())),
            ("Call sign", trimmed(m.call_sign.as_ref())),
            ("Destination", trimmed(m.destination.as_ref())),
            ("Position", position(m.lat, m.lon)),
            ("Speed over ground", knots(m.sog_kt)),
            ("Course over ground", degrees(m.cog_deg)),
            ("Heading", degrees(m.heading_deg.map(f64::from))),
            (
                "Navigational status",
                m.nav_status.map(|status| status.to_string()),
            ),
        ],
        some(&m.nmea),
    )
}

fn aprs(p: &AprsPacket) -> EventDetail {
    detail(
        [
            ("Source", some(&p.source)),
            ("Destination", some(&p.destination)),
            ("Path", some(p.path.join(" → "))),
            ("Symbol", p.symbol.clone()),
            ("Position", position(p.lat, p.lon)),
            ("Course", degrees(p.course_deg)),
            ("Speed", knots(p.speed_kt)),
            ("Altitude", feet(p.altitude_ft)),
            ("Mic-E message", p.mic_e_message.clone()),
            ("Comment", trimmed(p.comment.as_ref())),
        ],
        some(&p.tnc2),
    )
}

fn wsjt(m: &WsjtMessage) -> EventDetail {
    detail(
        [
            (
                "SNR",
                some(format!("{} dB", signed(f64::from(m.snr_db), 0))),
            ),
            ("Audio", some(format!("{:.1} Hz", m.audio_hz))),
            (
                "Time offset",
                some(format!("{} s", signed(f64::from(m.time_offset_s), 2))),
            ),
            ("Hard errors", some(m.hard_errors.to_string())),
        ],
        some(&m.text),
    )
}

fn wspr(s: &WsprSpot) -> EventDetail {
    detail(
        [
            ("Callsign", some(&s.callsign)),
            ("Grid", s.grid.clone()),
            ("Power", some(format!("{} dBm", s.power_dbm))),
            (
                "SNR",
                some(format!("{} dB", signed(f64::from(s.snr_db), 0))),
            ),
            ("Audio", some(format!("{:.1} Hz", s.audio_hz))),
            (
                "Time offset",
                some(format!("{} s", signed(f64::from(s.time_offset_s), 2))),
            ),
            (
                "Drift",
                some(format!("{} Hz", signed(f64::from(s.drift_hz), 1))),
            ),
        ],
        some(&s.text),
    )
}

fn selcall_plan(system: SelcallSystem) -> &'static str {
    match system {
        SelcallSystem::Ccir1 => "CCIR-1",
        SelcallSystem::Zvei1 => "ZVEI-1",
    }
}

fn navtex(n: &NavtexMessage) -> EventDetail {
    detail(
        [
            ("Header", n.header()),
            ("Station", n.station.map(String::from)),
            (
                "Subject",
                n.subject_name
                    .clone()
                    .or_else(|| n.subject.map(String::from)),
            ),
            ("Serial", n.serial.map(|serial| format!("{serial:02}"))),
            (
                "Ended with NNNN",
                some(if n.complete {
                    "yes"
                } else {
                    "no: flushed early"
                }),
            ),
            ("Repaired", repaired(n.errors_corrected, " characters")),
        ],
        some(&n.text),
    )
}

fn acars(a: &AcarsMessage) -> EventDetail {
    detail(
        [
            ("Registration", some(&a.registration)),
            ("Flight", trimmed(a.flight.as_ref())),
            ("Label", some(&a.label)),
            ("Mode", some(a.mode.to_string())),
            ("Block", some(a.block_id.to_string())),
            (
                "Direction",
                some(if a.downlink { "downlink" } else { "uplink" }),
            ),
            ("Sequence", trimmed(a.seq_no.as_ref())),
            (
                "Acknowledges",
                some(a.ack.map_or_else(|| "NAK".to_owned(), String::from)),
            ),
            (
                "Continues",
                a.more.then(|| "yes: another block follows".to_owned()),
            ),
        ],
        some(&a.text),
    )
}

fn reading_rows(reading: Option<&SubghzReading>) -> Vec<Row> {
    let unit = |value: Option<f64>, write: fn(f64) -> String| value.map(write);
    let Some(r) = reading else {
        return Vec::new();
    };
    vec![
        ("Model", some(&r.model)),
        ("Sensor id", some(bare_hex(u64::from(r.id), 2))),
        ("Channel", r.channel.map(|channel| channel.to_string())),
        (
            "Temperature",
            unit(r.temperature_c, |c| format!("{c:.1} °C")),
        ),
        (
            "Humidity",
            unit(r.humidity_pct, |pct| format!("{pct:.0} %")),
        ),
        (
            "Soil moisture",
            unit(r.moisture_pct, |pct| format!("{pct:.0} %")),
        ),
        (
            "Tyre pressure",
            unit(r.pressure_kpa, |kpa| format!("{kpa:.0} kPa")),
        ),
        (
            "Wind average",
            unit(r.wind_avg_kmh, |kmh| format!("{kmh:.1} km/h")),
        ),
        (
            "Wind gust",
            unit(r.wind_max_kmh, |kmh| format!("{kmh:.1} km/h")),
        ),
        (
            "Wind direction",
            unit(r.wind_dir_deg, |deg| format!("{deg:.0}°")),
        ),
        ("Rain", unit(r.rain_mm, |mm| format!("{mm:.1} mm"))),
        ("Power", unit(r.power_w, |w| format!("{w:.0} W"))),
        ("Energy", unit(r.energy_kwh, |kwh| format!("{kwh:.2} kWh"))),
        ("Battery", flag(r.battery_ok)),
    ]
}

fn subghz(f: &SubghzFrame) -> EventDetail {
    let mut rows = reading_rows(f.reading.as_ref());
    rows.extend([
        ("Modulation", some(serde_name(&f.modulation))),
        ("Encoding", some(serde_name(&f.encoding))),
        (
            "Payload",
            (f.bits > 0).then(|| format!("{} ({} bit)", f.data, f.bits)),
        ),
        (
            "EV1527 address",
            f.address.map(|a| bare_hex(u64::from(a), 5)),
        ),
        ("EV1527 button", f.button.map(|b| format!("{b:X}"))),
        ("PT2262 tri-state", f.tri_state.clone()),
        (
            "Base period",
            (f.short_us > 0).then(|| format!("{} µs", f.short_us)),
        ),
        (
            "Repeats",
            (f.repeats > 1).then(|| format!("×{}", f.repeats)),
        ),
    ]);
    detail(rows, timings(&f.timings_us))
}

#[must_use]
pub fn timings(us: &[u32]) -> Option<String> {
    let pairs: Vec<String> = us
        .chunks(2)
        .map(|pair| match pair {
            [pulse, gap] => format!("{pulse}/{gap}"),
            [pulse] => pulse.to_string(),
            _ => String::new(),
        })
        .collect();
    (!pairs.is_empty()).then(|| pairs.join("  "))
}

fn ident(r: &IdentReport) -> EventDetail {
    let Some(loudest) = r.loudest() else {
        let mut fields = vec![("Modulation", "no signal".to_owned())];
        fields.extend(ident_overview(r));
        return EventDetail { fields, body: None };
    };
    let mut fields = vec![
        ("Signals", r.signals.len().to_string()),
        ("Frequency", signal_frequency(loudest)),
        (
            "Modulation",
            modulation_label(loudest.modulation, loudest.sideband),
        ),
        ("Confidence", percent(f64::from(loudest.confidence))),
    ];
    fields.extend(ident_measurements(loudest));
    if loudest.features.frequency_levels > 1 {
        fields.push((
            "Frequency levels",
            loudest.features.frequency_levels.to_string(),
        ));
    }
    let body = r
        .signals
        .iter()
        .map(|signal| {
            let candidates: Vec<String> = signal
                .candidates
                .iter()
                .map(|m| format!("{}: {}, {}", m.name, candidate_score(m), m.why))
                .collect();
            format!(
                "{} · {}\n{}",
                signal_frequency(signal),
                modulation_label(signal.modulation, signal.sideband),
                candidates.join("\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    EventDetail {
        fields,
        body: Some(body),
    }
}

fn call(c: &VoiceCall) -> EventDetail {
    let destination = c.destination.map(|id| match c.group_call {
        Some(false) => format!("radio {id}"),
        _ => format!("talkgroup {id}"),
    });
    detail(
        [
            ("Mode", some(c.mode.label().to_uppercase())),
            ("Destination", destination),
            ("Source", c.source.map(|id| id.to_string())),
            ("Timeslot", c.slot.map(|slot| slot.to_string())),
            ("Colour code", c.color_code.map(|cc| cc.to_string())),
            (
                "Duration",
                some(format!("{:.1} s", c.duration_ms as f64 / 1000.0)),
            ),
            ("Started", some(&c.started_at)),
            ("Emergency", c.emergency.then(|| "yes".to_owned())),
            ("Encrypted", c.encrypted.then(|| "yes".to_owned())),
            ("Audio", c.audio_error.clone()),
        ],
        None,
    )
}

fn dv_kind(kind: DvFrameKind) -> &'static str {
    match kind {
        DvFrameKind::Header => "call",
        DvFrameKind::Voice => "in progress",
        DvFrameKind::Terminator => "end",
        DvFrameKind::Control => "signalling",
        DvFrameKind::Data => "data",
    }
}

fn dv_vendor(f: &DvFrame) -> Option<String> {
    let vendor = f.vendor?;
    let mfid = f
        .manufacturer_id
        .map_or_else(String::new, |id| format!(" ({})", hex(u64::from(id), 2)));
    Some(format!("{}{mfid}", vendor.label()))
}

fn slot_activity(f: &DvFrame) -> Option<String> {
    let items: Vec<String> = f
        .slot_activity
        .iter()
        .map(|item| {
            let hash = item
                .destination_hash
                .map_or_else(String::new, |h| format!(" (hash {})", hex(u64::from(h), 2)));
            format!("TS{} {}{hash}", item.slot, item.activity)
        })
        .collect();
    (!items.is_empty()).then(|| items.join(", "))
}

fn dv(f: &DvFrame) -> EventDetail {
    let definition = f.channel_definition.as_ref().map(|d| {
        format!(
            "LCN {} · TX {} · RX {}",
            d.channel,
            hertz(d.tx_hz as f64),
            hertz(d.rx_hz as f64)
        )
    });
    let call = f
        .group_call
        .map(|group| if group { "talkgroup" } else { "private" }.to_owned());
    detail(
        [
            ("Mode", some(f.mode.label())),
            ("Frame", some(dv_kind(f.kind))),
            ("Network", some(dv_network(f))),
            ("Vendor", dv_vendor(f)),
            ("Trunking", dv_trunking(f)),
            ("Parties", f.parties()),
            ("Talker alias", f.talker_alias.clone()),
            ("Call", call),
            ("Via", f.via.clone()),
            ("Signalling", f.opcode.clone()),
            ("Position", position(f.lat, f.lon)),
            (
                "Position error",
                f.position_error_m.map(|m| format!("≤ {m} m")),
            ),
            ("Channel", f.channel.map(|channel| channel.to_string())),
            ("Channel frequency", definition),
            (
                "Rest channel",
                f.rest_channel.map(|channel| channel.to_string()),
            ),
            ("Network ID", f.network_id.map(|id| id.to_string())),
            ("System ID", f.system_id.map(|id| id.to_string())),
            ("Site ID", f.site_id.map(|id| id.to_string())),
            ("Emergency", flag(f.emergency)),
            ("Late entry", flag(f.late_entry)),
            ("Slot activity", slot_activity(f)),
            ("Encrypted", flag(f.encrypted)),
            ("Algorithm", f.algorithm_id.map(|id| hex(u64::from(id), 2))),
            ("Key ID", f.key_id.map(|id| hex(u64::from(id), 4))),
            ("Message indicator", f.message_indicator.clone()),
            ("Repaired", repaired(f.errors_corrected, " bits")),
            ("Checksum", dv_checksum(f)),
        ],
        f.text.clone().or_else(|| f.data.clone()),
    )
}

fn broadcast_data(data: &BroadcastData) -> EventDetail {
    let body = (data.media_type == "text/plain")
        .then(|| String::from_utf8_lossy(&data.bytes).into_owned());
    EventDetail {
        fields: vec![
            ("Name", data.name.clone()),
            ("Type", data.media_type.clone()),
            ("Size", format!("{} bytes", data.bytes.len())),
        ],
        body,
    }
}

fn count(value: u32) -> Option<String> {
    (value > 0).then(|| value.to_string())
}

fn broadcast(status: &BroadcastStatus) -> EventDetail {
    let locked = status.locked;
    detail(
        [
            ("System", some(status.system.label())),
            ("Lock", some(if locked { "locked" } else { "searching" })),
            ("SNR", locked.then(|| format!("{:.1} dB", status.snr_db))),
            (
                "Frequency error",
                locked.then(|| format!("{} Hz", signed(f64::from(status.frequency_error_hz), 0))),
            ),
            (
                "Symbol rate",
                status.symbol_rate.map(|rate| format!("{rate} Bd")),
            ),
            (
                "Ensemble ID",
                status.ensemble_id.map(|id| hex(u64::from(id), 4)),
            ),
            (
                "Service ID",
                status.service_id.map(|id| hex(u64::from(id), 4)),
            ),
            ("Label", status.label.clone()),
            ("Audio frames", count(status.audio_frames_ok)),
            ("Audio failures", count(status.audio_frames_bad)),
            ("Audio error", status.audio_error.clone()),
            ("Video frames", count(status.video_frames_ok)),
            ("Video failures", count(status.video_frames_bad)),
            ("Video error", status.video_error.clone()),
            ("Data groups", count(status.data_groups_ok)),
            ("Data failures", count(status.data_groups_bad)),
            ("Data error", status.data_error.clone()),
            ("Dynamic label", status.dynamic_label.clone()),
        ],
        None,
    )
}

#[must_use]
pub fn utc_offset(minutes: Option<i16>) -> Option<String> {
    let minutes = minutes?;
    let sign = if minutes < 0 { "−" } else { "+" };
    let absolute = minutes.unsigned_abs();
    Some(format!(
        "UTC{sign}{:02}:{:02}",
        absolute / 60,
        absolute % 60
    ))
}

fn radio_clock(r: &RadioClockFrame) -> EventDetail {
    detail(
        [
            ("Service", some(serde_name(&r.standard).to_uppercase())),
            ("Civil time", some(&r.datetime)),
            ("UTC offset", utc_offset(r.utc_offset_minutes)),
            (
                "Daylight saving",
                some(if r.dst { "active" } else { "inactive" }),
            ),
            ("Leap warning", r.leap_warning.then(|| "yes".to_owned())),
            ("DUT1", r.dut1_seconds.map(|s| format!("{s:.1} s"))),
        ],
        some(&r.symbols),
    )
}

fn gnss(g: &GnssFrame) -> EventDetail {
    detail(
        [
            ("Signal", some(format!("GPS L1 C/A PRN {}", g.prn))),
            ("Doppler", some(format!("{:.0} Hz", g.doppler_hz))),
            (
                "Code phase",
                some(format!("{:.2} chips", g.code_phase_chips)),
            ),
            ("C/N₀", some(format!("{:.1} dB-Hz", g.cn0_db_hz))),
            (
                "NAV subframe",
                some(
                    g.subframe
                        .map_or_else(|| "acquiring telemetry".to_owned(), |s| s.to_string()),
                ),
            ),
            ("Time of week", g.tow_seconds.map(|s| format!("{s} s"))),
            ("GPS week (10 bit)", g.week.map(|week| week.to_string())),
        ],
        some(g.words.join(" ")),
    )
}

fn vor(v: &VorReading) -> EventDetail {
    detail(
        [
            ("Station", v.station.clone()),
            ("Radial", some(format!("{:.2}°", v.radial_deg))),
            (
                "Variable phase",
                some(format!("{:.2}°", v.variable_phase_deg)),
            ),
            (
                "Reference phase",
                some(format!("{:.2}°", v.reference_phase_deg)),
            ),
            (
                "Magnetic declination",
                some(format!("{}°", signed(v.magnetic_declination_deg, 1))),
            ),
            ("Station position", position(v.station_lat, v.station_lon)),
            ("Signal", some(format!("{:.1} dB", v.signal_db))),
            ("Confidence", some(percent(f64::from(v.confidence)))),
        ],
        None,
    )
}

fn data_link(m: &DataLinkMessage) -> EventDetail {
    let details = match &m.details {
        serde_json::Value::Null => None,
        serde_json::Value::Object(map) if map.is_empty() => None,
        other => serde_json::to_string_pretty(other).ok(),
    };
    let text = m
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let body: Vec<String> = [text, details].into_iter().flatten().collect();
    detail(
        [
            ("Message type", some(&m.message_type)),
            ("Station", m.station.clone()),
            (
                "Integrity",
                some(if m.crc_ok { "verified" } else { "failed" }),
            ),
            ("FEC repaired", m.fec_corrected.map(|n| n.to_string())),
            ("SNR", m.snr_db.map(|db| format!("{db:.1} dB"))),
            (
                "Frequency error",
                m.frequency_error_hz
                    .map(|hz| format!("{} Hz", signed(f64::from(hz), 1))),
            ),
            ("Position", position(m.lat, m.lon)),
            ("Raw", m.raw.clone()),
        ],
        some(body.join("\n\n")),
    )
}

#[must_use]
pub fn dect_carriers(mask: Option<u16>) -> Option<String> {
    let mask = mask?;
    let on: Vec<String> = (0..10)
        .filter(|carrier| (mask >> (9 - carrier)) & 1 == 1)
        .map(|carrier| carrier.to_string())
        .collect();
    (!on.is_empty()).then(|| on.join(", "))
}

fn dect(frame: &DectFrame) -> EventDetail {
    let id = frame.identity.as_ref();
    let hex4 = |value: Option<u16>| value.map(|v| bare_hex(u64::from(v), 4));
    let handsets: Vec<String> = frame
        .handsets
        .iter()
        .map(|handset| bare_hex(u64::from(*handset), 5))
        .collect();
    let capabilities: Vec<&str> = frame.capabilities.iter().map(|c| c.label()).collect();
    let rows = [
        (
            "Side",
            some(if frame.side == DectSide::Rfp {
                "base station"
            } else {
                "handset"
            }),
        ),
        ("RFPI", id.map(|id| id.rfpi.clone())),
        (
            "Access rights class",
            id.map(|id| id.arc.label().to_owned()),
        ),
        ("PARI", id.map(|id| id.pari.clone())),
        ("Manufacturer code", hex4(id.and_then(|id| id.emc))),
        ("Installer code", hex4(id.and_then(|id| id.eic))),
        ("Operator code", hex4(id.and_then(|id| id.poc))),
        (
            "GSM/UMTS operator",
            id.and_then(|id| id.gop).map(|g| bare_hex(u64::from(g), 5)),
        ),
        (
            "Fixed part number",
            id.and_then(|id| id.fpn).map(|n| n.to_string()),
        ),
        (
            "Fixed part sub-number",
            id.and_then(|id| id.fps).map(|n| n.to_string()),
        ),
        ("Radio fixed part", id.map(|id| id.rpn.to_string())),
        (
            "Cell",
            id.and_then(|id| id.multicell)
                .map(|multi| if multi { "multi-cell" } else { "single cell" }.to_owned()),
        ),
        ("SARI list", flag(id.map(|id| id.sari_available))),
        ("Carrier", frame.carrier.map(|c| c.to_string())),
        ("Frequency", frame.carrier_hz.map(hertz)),
        ("Slot pair", frame.slot_pair.map(|s| s.to_string())),
        ("Transceivers", frame.transceivers.map(|t| t.to_string())),
        ("Carriers available", dect_carriers(frame.rf_carriers)),
        ("Scan carrier", frame.pscn.map(|p| p.to_string())),
        ("Multiframe", frame.multiframe.map(|m| m.to_string())),
        (
            "Authentication",
            flag(frame.security.authentication_supported),
        ),
        ("Ciphering", flag(frame.security.ciphering_supported)),
        ("Encryption", some(frame.security.cipher_state.label())),
        ("Last cipher command", frame.security.last_command.clone()),
        (
            "Cipher key index",
            frame.security.cipher_key_index.map(|k| k.to_string()),
        ),
        ("FMID", hex4(frame.fmid)),
        ("PMID", frame.pmid.map(|p| bare_hex(u64::from(p), 5))),
        ("Handsets", some(handsets.join(", "))),
        ("Bursts", some(frame.bursts.to_string())),
        ("A-field CRC errors", some(frame.crc_errors.to_string())),
        ("Level", some(format!("{:.1} dBFS", frame.level_dbfs))),
    ];
    detail(rows, some(capabilities.join("\n")))
}

#[cfg(test)]
mod tests;

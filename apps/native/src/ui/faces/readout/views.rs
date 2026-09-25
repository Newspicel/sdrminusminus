use sdrmm_wire::{
    decode::{DecoderEvent, IdentSignal},
    units::hertz,
};
use zgui::prelude::*;

use crate::{
    decoders::views::{
        CW_SPOT_TEXT_LIMIT, DecoderScope, RdsGrade, candidate_score, cw_signal_rows, dect_stations,
        format_alt_freqs, format_clock, ident_measurements, ident_overview, latest_vor_readings,
        modulation_label, multi_vor_fix, pty_label, rds_picture, rds_quality, signal_frequency,
        tone_label, vor_of,
    },
    store::Store,
    ui::kit_decoders::{self, fields_view},
};

use super::{frames_of, pictures, records, targets, text};

pub fn decoder_view(store: Store, kind: &str, scope: DecoderScope) -> Option<AnyView> {
    Some(match kind {
        "rds" => AnyView::new(rds_view(store, scope)),
        "adsb" => AnyView::new(targets::view(store, true, scope)),
        "ais" => AnyView::new(targets::view(store, false, scope)),
        "rtty" => AnyView::new(text::view(store, "rtty", scope)),
        "morse" => AnyView::new(text::view(store, "morse", scope)),
        "psk" => AnyView::new(text::view(store, "psk", scope)),
        "cw_skimmer" => AnyView::new(cw_view(store, scope)),
        "tone" => AnyView::new(tone_view(store, scope)),
        "ident" => AnyView::new(ident_view(store, scope)),
        "dect" => AnyView::new(dect_view(store, scope)),
        "vor" => AnyView::new(vor_view(store, scope)),
        "sstv" => AnyView::new(pictures::view(store, scope)),
        "broadcast" | "broadcast_data" => AnyView::new(broadcast_view(store, scope)),
        _ => return None,
    })
}

fn empty(said: &'static str) -> AnyView {
    AnyView::new(view! { text(class = "hint") {{said}} })
}

fn chip(label: String, on: bool) -> impl IntoView {
    view! { text(class = "dk-chip", class:on = on, class:off = !on) {{label}} }
}

fn rds_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "rds");
    move || {
        let Some(rds) = rds_picture(&records(frames, scope)) else {
            return empty("No RDS yet: tune a WFM station that carries it.");
        };
        let quality = rds_quality(&rds);
        let grade = match quality.grade {
            RdsGrade::Good => "dk-accent",
            RdsGrade::Fair => "",
            RdsGrade::NoLock | RdsGrade::Poor => "dk-danger",
        };
        let ps = rds
            .ps
            .as_deref()
            .map(str::trim)
            .filter(|ps| !ps.is_empty())
            .unwrap_or("········")
            .to_owned();
        let radiotext = rds
            .radiotext
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("-")
            .to_owned();
        let alternatives = format_alt_freqs(&rds.alt_freqs_hz);
        let chips: Vec<AnyView> = alternatives
            .into_iter()
            .map(|af| AnyView::new(chip(af, false)))
            .collect();
        let pi = format!("PI {}", rds.pi.as_deref().unwrap_or("-"));
        let music = if rds.music == Some(false) { "SP" } else { "MS" };
        let counts = format!(
            "{} groups · {} block errors · {:.1}%",
            quality.groups,
            quality.block_errors,
            quality.error_rate * 100.0
        );
        AnyView::new(view! {
            column(class = "dk-pane") {
                row(class = "dk-line") {
                    text(class = "dk-big") {{ps}}
                    text(class = "dk-num") {{pi}}
                    text(class = "dk-dim") {{pty_label(&rds)}}
                    row(class = "dk-line dk-push") {
                        {chip("TP".to_owned(), rds.tp == Some(true))}
                        {chip("TA".to_owned(), rds.ta == Some(true))}
                        {chip(music.to_owned(), rds.music.is_some())}
                    }
                }
                text(class = "legend") {"RadioText"}
                text(class = "dk-box") {{radiotext}}
                row(class = "dk-line") {
                    text(class = "legend") {"AF"}
                    {chips}
                    row(class = "dk-line dk-push") {
                        text(class = "legend") {"Quality"}
                        text(class = format!("dk-num {grade}")) {{quality.grade.label()}}
                        text(class = "dk-num") {{counts}}
                    }
                }
            }
        })
    }
}

fn table_row(cells: Vec<String>) -> impl IntoView {
    let cells: Vec<_> = cells
        .into_iter()
        .map(|cell| view! { text(class = "dk-td") {{cell}} })
        .collect();
    view! { row(class = "dk-tr") {{cells}} }
}

fn table_head(names: &[&'static str]) -> impl IntoView + use<> {
    let cells: Vec<_> = names
        .iter()
        .map(|name| view! { text(class = "dk-td dk-th") {{*name}} })
        .collect();
    view! { row(class = "dk-tr") {{cells}} }
}

fn cw_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "cw_skimmer");
    move || {
        let rows = cw_signal_rows(&records(frames, scope), CW_SPOT_TEXT_LIMIT);
        if rows.is_empty() {
            return empty("No CW carriers decoded yet.");
        }
        let count = rows.len().to_string();
        let body: Vec<AnyView> = rows
            .into_iter()
            .map(|row| {
                AnyView::new(table_row(vec![
                    hertz(row.frequency_hz),
                    format!("{:+.0} Hz", row.offset_hz),
                    format!("{:.0} WPM", row.wpm),
                    format!("{:.0} dB", row.snr_db),
                    row.text,
                ]))
            })
            .collect();
        AnyView::new(view! {
            column(class = "dk-pane") {
                row(class = "dk-line") {
                    text(class = "legend") {"Signals in passband"}
                    text(class = "dk-num") {{count}}
                }
                column(class = "dk-table") {
                    {table_head(&["Frequency", "Offset", "Speed", "SNR", "Text"])}
                    {body}
                }
            }
        })
    }
}

fn tone_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "tone");
    move || {
        let latest = records(frames, scope).into_iter().next();
        let Some((at, DecoderEvent::Tone(status))) =
            latest.map(|record| (record.at.clone(), record.event.clone()))
        else {
            return empty("No subaudible tone heard.");
        };
        let label = tone_label(status.ctcss_hz, status.dcs_code);
        let label = if label.is_empty() {
            "no tone".to_owned()
        } else {
            label
        };
        AnyView::new(view! {
            row(class = "dk-line") {
                text(class = "dk-num") {{format_clock(&at)}}
                text(class = "dk-num dk-accent") {{label}}
                text(class = "legend") {{if status.open { "open" } else { "muted" }}}
            }
        })
    }
}

fn ident_signal(signal: &IdentSignal) -> impl IntoView + use<> {
    let candidates: Vec<_> = signal
        .candidates
        .iter()
        .map(|candidate| {
            let name_class = if candidate.confirmed {
                "dk-mid dk-accent"
            } else {
                "dk-mid"
            };
            view! {
                row(class = "dk-line") {
                    text(class = name_class) {{candidate.name.clone()}}
                    text(class = "dk-chip") {{candidate_score(candidate)}}
                    text(class = "dk-dim") {{candidate.why.clone()}}
                }
            }
        })
        .collect();
    let none = signal
        .candidates
        .is_empty()
        .then(|| view! { text(class = "hint") {"Nothing in the catalog fits."} });
    view! {
        column(class = "dk-pane") {
            row(class = "dk-line") {
                text(class = "dk-num") {{signal_frequency(signal)}}
                text(class = "dk-mid") {{modulation_label(signal.modulation, signal.sideband)}}
                text(class = "legend") {{format!("{:.0}% confident", signal.confidence * 100.0)}}
            }
            {fields_view(ident_measurements(signal))}
            text(class = "legend") {"Protocol"}
            {candidates}
            {none}
        }
    }
}

fn ident_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "ident");
    move || {
        let latest = records(frames, scope).into_iter().next();
        let Some((at, DecoderEvent::Ident(report))) =
            latest.map(|record| (record.at.clone(), record.event.clone()))
        else {
            return empty("Nothing analysed yet.");
        };
        let count = report.signals.len();
        let headline = match count {
            0 => "no signal".to_owned(),
            1 => "1 signal".to_owned(),
            n => format!("{n} signals"),
        };
        let signals: Vec<AnyView> = report
            .signals
            .iter()
            .map(|s| AnyView::new(ident_signal(s)))
            .collect();
        AnyView::new(view! {
            column(class = "dk-pane") {
                row(class = "dk-line") {
                    text(class = "dk-big") {{headline}}
                    text(class = "dk-num dk-push") {{format_clock(&at)}}
                }
                {fields_view(ident_overview(&report))}
                {signals}
            }
        })
    }
}

fn support(value: Option<bool>) -> String {
    value
        .map_or("-", |on| if on { "yes" } else { "no" })
        .to_owned()
}

fn dect_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "dect");
    move || {
        let stations = dect_stations(&records(frames, scope));
        if stations.is_empty() {
            return empty("No DECT base stations heard yet.");
        }
        let rows: Vec<AnyView> = stations
            .into_iter()
            .map(|station| {
                let carrier = match (station.carrier, station.carrier_hz) {
                    (None, _) => "-".to_owned(),
                    (Some(carrier), None) => carrier.to_string(),
                    (Some(carrier), Some(hz)) => format!("{carrier} · {}", hertz(hz)),
                };
                let bad = if station.crc_errors == 0 {
                    String::new()
                } else {
                    format!(" / {} bad", station.crc_errors)
                };
                AnyView::new(table_row(vec![
                    station.rfpi.unwrap_or_else(|| "-".to_owned()),
                    station.arc.map_or("-".to_owned(), |arc| {
                        crate::decoders::text::serde_name(&arc).to_uppercase()
                    }),
                    carrier,
                    station
                        .slot_pair
                        .map_or("-".to_owned(), |slot| slot.to_string()),
                    support(station.authentication),
                    support(station.ciphering),
                    station.cipher_state.label().to_owned(),
                    if station.handsets == 0 {
                        "-".to_owned()
                    } else {
                        station.handsets.to_string()
                    },
                    format!("{:.1}", station.level_dbfs),
                    format!("{}{bad}", station.bursts),
                ]))
            })
            .collect();
        AnyView::new(view! {
            column(class = "dk-table") {
                {table_head(&["RFPI", "Class", "Carrier", "Slot", "Auth", "Cipher", "State", "Handsets", "dBFS", "Bursts"])}
                {rows}
            }
        })
    }
}

fn vor_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let frames = frames_of(store, "vor");
    move || {
        let latest = latest_vor_readings(&records(frames, scope));
        if latest.is_empty() {
            return empty("No VOR reports yet.");
        }
        let readings: Vec<_> = latest.iter().filter_map(|record| vor_of(record)).collect();
        let fix = multi_vor_fix(&readings).map_or_else(
            || {
                AnyView::new(
                    view! { text(class = "hint") {"Two VORs with coordinates give a fix."} },
                )
            },
            |fix| {
                let residual = if fix.residual_km < 1.0 {
                    format!("{} m residual", (fix.residual_km * 1000.0).round())
                } else {
                    format!("{:.1} km residual", fix.residual_km)
                };
                AnyView::new(view! {
                    row(class = "dk-line") {
                        text(class = "dk-big") {{format!("{:.5}, {:.5}", fix.lat, fix.lon)}}
                        text(class = "dk-chip") {{format!("{} stations", fix.stations)}}
                        text(class = "dk-dim") {{residual}}
                    }
                })
            },
        );
        let rows: Vec<AnyView> = latest
            .iter()
            .filter_map(|record| {
                let reading = vor_of(record)?;
                Some(AnyView::new(table_row(vec![
                    reading
                        .station
                        .clone()
                        .unwrap_or_else(|| format!("D{} C{}", record.device_set, record.channel)),
                    format!("{:.1}°", reading.radial_deg),
                    format!("{:.0}%", reading.confidence * 100.0),
                    format!("{:.1} dB", reading.signal_db),
                ])))
            })
            .collect();
        AnyView::new(view! {
            column(class = "dk-pane") {
                {fix}
                column(class = "dk-table") {
                    {table_head(&["Station", "Radial", "Confidence", "Signal"])}
                    {rows}
                }
            }
        })
    }
}

fn broadcast_view(store: Store, scope: DecoderScope) -> impl IntoView {
    let statuses = frames_of(store, "broadcast");
    let objects = frames_of(store, "broadcast_data");
    move || {
        let newest = records(statuses, scope).into_iter().next();
        let status = newest.as_ref().and_then(|record| match &record.event {
            DecoderEvent::Broadcast(status) => Some(status.clone()),
            _ => None,
        });
        let object = newest
            .as_ref()
            .zip(status.as_ref())
            .filter(|(_, s)| s.locked)
            .and_then(|(record, status)| {
                records(objects, scope)
                    .into_iter()
                    .find_map(|candidate| match &candidate.event {
                        DecoderEvent::BroadcastData(data)
                            if data.service_id == status.service_id
                                && candidate.device_set == record.device_set
                                && candidate.channel == record.channel
                                && candidate.freq_hz == record.freq_hz =>
                        {
                            Some(data.clone())
                        }
                        _ => None,
                    })
            });
        if status.is_none() && object.is_none() {
            return empty("Waiting for a broadcast service.");
        }
        let label = status
            .as_ref()
            .and_then(|status| status.label.clone())
            .unwrap_or_else(|| "Broadcast service".to_owned());
        let dynamic = status
            .as_ref()
            .and_then(|status| status.dynamic_label.clone())
            .map(|text| AnyView::new(view! { text(class = "dk-mid") {{text}} }));
        let object =
            object.map(|data| AnyView::new(kit_decoders::broadcast_data_view(store, &data)));
        AnyView::new(view! {
            column(class = "dk-pane") {
                text(class = "legend") {{label}}
                {dynamic}
                {object}
            }
        })
    }
}

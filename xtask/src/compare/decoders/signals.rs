use sdrmm_wire::DecoderEvent;

use super::{
    convert::{Audio, Container, Demod, Format},
    parse,
    programs::{
        ACARSDEC, AIS_CATCHER, DIREWOLF, DSD_FME, DUMP1090, FT8_LIB, MULTIMON, Program, READSB,
    },
};

pub struct Signal {
    pub id: &'static str,
    pub name: &'static str,
    pub fixture: &'static str,
    pub channel: &'static str,
    pub offset_hz: f64,
    pub speed_seconds: f64,
    pub tail_seconds: f64,
    pub cu8_rate: Option<f64>,
    pub unique: bool,
    pub decodes: Show,
    pub speed: Show,
    pub key: fn(&DecoderEvent) -> Option<String>,
    pub references: &'static [Reference],
}

#[derive(Clone, Copy)]
pub enum Show {
    Hidden,
    Plain,
    Noted(&'static str),
}

pub struct Reference {
    pub program: &'static Program,
    pub format: Format,
    pub args: &'static [&'static str],
    pub exit_codes: &'static [i32],
    pub tail: bool,
    pub keys: fn(&str) -> Vec<String>,
}

impl Signal {
    pub fn loops(&self, fixture_seconds: f64) -> usize {
        ((self.speed_seconds / fixture_seconds).ceil() as usize).max(1)
    }
}

pub const INPUT: &str = "{input}";

const FROM_FM: &str = "Others start from FM audio, SDR-- from IQ.";

const MODE_S: Format = Format::Cu8 { rate: 2_400_000.0 };

const fn fm(deviation_hz: f64, rate: u32, container: Container) -> Format {
    Format::Audio(Audio {
        demod: Demod::Fm { deviation_hz },
        rate,
        container,
    })
}

pub const SIGNALS: &[Signal] = &[
    Signal {
        id: "adsb",
        name: "ADS-B",
        fixture: "adsb_offair_2m",
        channel: "adsb",
        offset_hz: 0.0,
        speed_seconds: 20.0,
        tail_seconds: 0.01,
        cu8_rate: Some(2_400_000.0),
        unique: true,
        decodes: Show::Hidden,
        speed: Show::Plain,
        key: adsb,
        references: &[
            Reference {
                program: &DUMP1090,
                format: MODE_S,
                args: &[
                    "--device-type",
                    "ifile",
                    "--ifile",
                    INPUT,
                    "--iformat",
                    "UC8",
                    "--raw",
                ],
                exit_codes: &[0, 1],
                tail: true,
                keys: parse::mode_s,
            },
            Reference {
                program: &READSB,
                format: MODE_S,
                args: &[
                    "--device-type",
                    "ifile",
                    "--ifile",
                    INPUT,
                    "--iformat",
                    "UC8",
                    "--raw",
                ],
                exit_codes: &[0, 1],
                tail: true,
                keys: parse::mode_s,
            },
        ],
    },
    Signal {
        id: "ais",
        name: "AIS",
        fixture: "ais_position_240k",
        channel: "ais",
        offset_hz: 25_000.0,
        speed_seconds: 30.0,
        tail_seconds: 0.1,
        cu8_rate: None,
        unique: true,
        decodes: Show::Hidden,
        speed: Show::Plain,
        key: ais,
        references: &[Reference {
            program: &AIS_CATCHER,
            format: Format::Cf32,
            args: &["-r", "CF32", INPUT, "-s", "240000", "-n"],
            exit_codes: &[0],
            tail: true,
            keys: parse::nmea_payloads,
        }],
    },
    Signal {
        id: "aprs",
        name: "APRS",
        fixture: "aprs_afsk1200_240k",
        channel: "aprs",
        offset_hz: -40_000.0,
        speed_seconds: 30.0,
        tail_seconds: 0.2,
        cu8_rate: None,
        unique: true,
        decodes: Show::Hidden,
        speed: Show::Noted(FROM_FM),
        key: aprs,
        references: &[
            Reference {
                program: &DIREWOLF,
                format: fm(3_000.0, 48_000, Container::Wav),
                args: &[INPUT],
                exit_codes: &[0],
                tail: true,
                keys: parse::direwolf,
            },
            Reference {
                program: &MULTIMON,
                format: fm(3_000.0, 22_050, Container::Raw),
                args: &["-q", "-a", "AFSK1200", "-A", "-t", "raw", INPUT],
                exit_codes: &[0],
                tail: true,
                keys: parse::multimon_aprs,
            },
        ],
    },
    Signal {
        id: "pocsag",
        name: "POCSAG",
        fixture: "pocsag_1200_240k",
        channel: "pocsag",
        offset_hz: 50_000.0,
        speed_seconds: 30.0,
        tail_seconds: 0.5,
        cu8_rate: None,
        unique: true,
        decodes: Show::Hidden,
        speed: Show::Noted(FROM_FM),
        key: pocsag,
        references: &[Reference {
            program: &MULTIMON,
            format: fm(4_500.0, 22_050, Container::Raw),
            args: &["-q", "-a", "POCSAG1200", "-t", "raw", INPUT],
            exit_codes: &[0],
            tail: true,
            keys: parse::multimon_pocsag,
        }],
    },
    Signal {
        id: "acars",
        name: "ACARS",
        fixture: "acars_offair_48k",
        channel: "acars",
        offset_hz: 0.0,
        speed_seconds: 30.0,
        tail_seconds: 0.5,
        cu8_rate: None,
        unique: true,
        decodes: Show::Plain,
        speed: Show::Noted("acarsdec starts from AM audio, SDR-- from IQ."),
        key: acars,
        references: &[Reference {
            program: &ACARSDEC,
            format: Format::Audio(Audio {
                demod: Demod::Am,
                rate: 12_500,
                container: Container::Wav,
            }),
            args: &["-o", "4", "-f", INPUT],
            exit_codes: &[0, 255],
            tail: true,
            keys: parse::acarsdec,
        }],
    },
    Signal {
        id: "ft8",
        name: "FT8",
        fixture: "ft8_20m_busy_12k",
        channel: "ft8",
        offset_hz: 0.0,
        speed_seconds: 0.0,
        tail_seconds: 4.0,
        cu8_rate: None,
        unique: true,
        decodes: Show::Noted("Two SDR-- decodes are unconfirmed."),
        speed: Show::Noted("SDR-- re-decodes overlapping windows."),
        key: ft8,
        references: &[Reference {
            program: &FT8_LIB,
            format: Format::Audio(Audio {
                demod: Demod::Real,
                rate: 12_000,
                container: Container::Wav,
            }),
            args: &[INPUT],
            exit_codes: &[0],
            tail: false,
            keys: parse::ft8_lib,
        }],
    },
    Signal {
        id: "dmr",
        name: "DMR",
        fixture: "dmr_call_48k",
        channel: "dmr",
        offset_hz: 0.0,
        speed_seconds: 30.0,
        tail_seconds: 0.5,
        cu8_rate: None,
        unique: false,
        decodes: Show::Noted("Counts frames carrying the call's addresses."),
        speed: Show::Noted(FROM_FM),
        key: dmr,
        references: &[Reference {
            program: &DSD_FME,
            format: fm(2_400.0, 48_000, Container::Wav),
            args: &["-fs", "-i", INPUT, "-o", "null"],
            exit_codes: &[0],
            tail: true,
            keys: parse::dsd_fme,
        }],
    },
];

fn adsb(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Adsb(message) => Some(message.raw.to_ascii_uppercase()),
        _ => None,
    }
}

fn ais(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Ais(message) => parse::nmea_payloads(&message.nmea).into_iter().next(),
        _ => None,
    }
}

fn aprs(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Aprs(packet) => Some(packet.tnc2.clone()),
        _ => None,
    }
}

fn pocsag(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Pocsag(message) => Some(format!("{} {}", message.address, message.text)),
        _ => None,
    }
}

fn acars(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Acars(message) => Some(format!(
            "{} {} {}",
            message.registration, message.label, message.text
        )),
        _ => None,
    }
}

fn ft8(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Ft8(message) => Some(message.text.clone()),
        _ => None,
    }
}

fn dmr(event: &DecoderEvent) -> Option<String> {
    match event {
        DecoderEvent::Dv(frame) => {
            let (source, destination) = (frame.source?, frame.destination?);
            Some(format!("{source} {destination}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_fixtures_loop_to_the_speed_length() {
        assert_eq!(SIGNALS[0].loops(0.2), 100);
        assert_eq!(SIGNALS[0].loops(30.0), 1);
        let ft8 = SIGNALS.iter().find(|s| s.id == "ft8").expect("ft8");
        assert_eq!(ft8.loops(15.0), 1);
    }

    #[test]
    fn every_reference_reads_its_input() {
        for signal in SIGNALS {
            for reference in signal.references {
                assert!(reference.args.contains(&INPUT), "{}", signal.id);
            }
        }
    }
}

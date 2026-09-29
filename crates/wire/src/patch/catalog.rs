use super::{
    ARRAY_LANE_PORT, ARRAY_PORT, CONTROL_PORT, EVENTS_PORT, NodeBody, NodeTypeInfo, POSITION_PORT,
    PROCESSOR_CATALOG, PatchCatalog, PortCondition, PortDirection, PortRepeat, PortSpec, PortType,
    processor_ports,
};

const FIXED_CATALOG: &[(&str, &str, &str)] = &[
    ("device", "Device", "A radio, where every patch starts"),
    ("recording", "Recording", "Plays back a recorded IQ file"),
    (
        "signal_gen",
        "Signal generator",
        "Test signals without a radio",
    ),
    (
        "gps",
        "GPS position",
        "Your station location, live or fixed",
    ),
    ("array", "Array", "Radio lanes as one antenna array"),
    ("channel", "Channel", "Tunes and decodes one signal"),
    ("scope", "Scope", "Spectrum and waterfall"),
    (
        "baseband_scope",
        "Baseband scope",
        "Constellation and eye of one channel",
    ),
    ("speaker", "Speaker", "Plays channel audio"),
    ("map", "Map", "Decoded positions on a map"),
    (
        "signal_map",
        "Signal survey",
        "Maps signal strength while you move",
    ),
    (
        "propagation",
        "Propagation map",
        "Where FT8, FT4 and WSPR signals came from",
    ),
    ("readout", "Readout", "Current decoder state"),
    (
        "decoder_log",
        "Decoder log",
        "Every decoded message in a table",
    ),
    (
        "spectrum_monitor",
        "Spectrum monitor",
        "Catches and decodes everything in view",
    ),
    (
        "dmr_trunk",
        "DMR trunk system",
        "Follows calls across a DMR trunk system",
    ),
    (
        "event_filter",
        "Event filter",
        "Passes only matching events",
    ),
    ("audio_fx", "Audio FX", "Filters, denoise and AGC"),
    (
        "event_output",
        "Event output",
        "Sends events to other programs",
    ),
    ("video", "Video", "ATV frames and SSTV pictures"),
    ("recorder", "Recorder", "Records a radio's full IQ"),
    (
        "audio_recorder",
        "Audio recorder",
        "Records channel audio to WAV",
    ),
    (
        "baseband_recorder",
        "Baseband recorder",
        "Records one channel's IQ",
    ),
    (
        "time_machine",
        "Time machine",
        "Saves IQ from before you pressed record",
    ),
    (
        "network_export",
        "Network IQ",
        "Streams IQ to other programs",
    ),
    ("export", "Export", "Saves logged rows as CSV or JSON"),
    (
        "scanner",
        "Scanner",
        "Steps through frequencies, stops on activity",
    ),
    ("hunt", "Signal hunt", "Walks you towards a transmitter"),
    (
        "satellite",
        "Satellite",
        "Predicts passes and follows Doppler",
    ),
];

const TRIANGULATION_CATALOG: (&str, &str, &str) = (
    "triangulation",
    "Triangulation",
    "Crosses bearings into a position",
);

pub(super) fn catalog_rows()
-> impl Iterator<Item = &'static (&'static str, &'static str, &'static str)> {
    FIXED_CATALOG
        .iter()
        .chain(PROCESSOR_CATALOG)
        .chain(std::iter::once(&TRIANGULATION_CATALOG))
}

impl PatchCatalog {
    #[must_use]
    pub fn build() -> Self {
        Self {
            nodes: catalog_rows()
                .filter_map(|&(kind, name, summary)| {
                    let body = NodeBody::default_for(kind)?;
                    Some(NodeTypeInfo {
                        kind: kind.to_owned(),
                        name: name.to_owned(),
                        summary: summary.to_owned(),
                        category: body.category(),
                        ports: ports_for(kind),
                        needs_channel_type: matches!(body, NodeBody::Channel(_)),
                        default_body: body,
                    })
                })
                .collect(),
        }
    }
}

pub(super) fn ports_for(kind: &str) -> Vec<PortSpec> {
    processor_ports(kind).unwrap_or_else(|| fixed_ports(kind))
}

fn fixed_ports(kind: &str) -> Vec<PortSpec> {
    use PortCondition::{
        Always, ChannelHasAudio, ChannelHasVideo, ChannelIsDecoder, ChannelNeedsPosition,
        DeviceIsTxCapable,
    };
    use PortDirection::{In, Out};
    use PortType::{Array, Audio, Baseband, Control, Events, Iq, Position, Tx, Video};
    match kind {
        "device" => vec![
            PortSpec::new(Tx, In, false, DeviceIsTxCapable)
                .repeated(PortRepeat::PerTxStream)
                .noted(
                    "reserved: transmit is not built (), so nothing in this build emits \
                     a signal to key a radio with",
                ),
            PortSpec::new(Iq, Out, true, Always).repeated(PortRepeat::PerRxStream),
        ],
        "recording" => {
            vec![PortSpec::new(Iq, Out, true, Always).repeated(PortRepeat::PerRxStream)]
        }
        "signal_gen" => vec![PortSpec::new(Iq, Out, true, Always)],
        "gps" => vec![PortSpec::new(Position, Out, true, Always)],
        "array" => vec![
            PortSpec::named(ARRAY_LANE_PORT, Iq, In, false)
                .repeated(PortRepeat::PerLane)
                .noted("One radio lane per antenna, in antenna order"),
            PortSpec::named(POSITION_PORT, Position, In, false).noted("Array place and heading"),
            PortSpec::named(ARRAY_PORT, Array, Out, true),
        ],
        "channel" => vec![
            PortSpec::new(Iq, In, true, Always)
                .noted("every radio that may carry this decoder; it runs on the one that hears it"),
            PortSpec::new(Control, In, false, Always).noted(
                "a scanner, signal hunt or satellite drives this decoder; its radio follows",
            ),
            PortSpec::new(Position, In, false, ChannelNeedsPosition),
            PortSpec::new(Baseband, Out, true, Always),
            PortSpec::new(Audio, Out, true, ChannelHasAudio),
            PortSpec::new(Events, Out, true, ChannelIsDecoder),
            PortSpec::new(Video, Out, true, ChannelHasVideo),
        ],
        "scope" => vec![PortSpec::new(Iq, In, false, Always)],
        "baseband_scope" => vec![PortSpec::new(Baseband, In, false, Always)],
        "recorder" | "time_machine" | "signal_map" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "audio_recorder" => vec![PortSpec::new(Audio, In, true, Always)],
        "baseband_recorder" => vec![PortSpec::new(Baseband, In, true, Always)],
        "network_export" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Baseband, In, false, Always),
        ],
        "scanner" => vec![PortSpec::new(Control, Out, false, Always)],
        "hunt" => vec![
            PortSpec::named(CONTROL_PORT, Control, Out, false),
            PortSpec::named(POSITION_PORT, Position, In, false)
                .noted("Where you stand and which way you point"),
            PortSpec::named(EVENTS_PORT, Events, Out, true).noted("Sweep and mark bearings"),
        ],
        "satellite" => vec![
            PortSpec::new(Position, In, false, Always),
            PortSpec::new(Control, Out, true, Always).noted(
                "every decoder listening to this satellite; each is tuned and Doppler corrected",
            ),
        ],
        "speaker" => vec![PortSpec::new(Audio, In, true, Always)],
        "video" => vec![PortSpec::new(Video, In, true, Always)],
        "map" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Position, In, true, Always),
        ],
        "propagation" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "readout" | "decoder_log" | "export" | "event_output" => {
            vec![PortSpec::new(Events, In, true, Always)]
        }
        "spectrum_monitor" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
        "dmr_trunk" => vec![
            PortSpec::new(Iq, In, false, Always)
                .noted("the radio the control channel sits on; the system runs its own decoders"),
            PortSpec::new(Events, Out, true, Always),
        ],
        "event_filter" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
        "audio_fx" => vec![
            PortSpec::new(Audio, In, true, Always),
            PortSpec::new(Audio, Out, true, Always),
        ],
        "triangulation" => vec![
            PortSpec::named(EVENTS_PORT, Events, In, true).noted("Bearings"),
            PortSpec::named(POSITION_PORT, Position, In, false).noted("Vehicle to guide"),
            PortSpec::named(EVENTS_PORT, Events, Out, true),
        ],
        _ => Vec::new(),
    }
}

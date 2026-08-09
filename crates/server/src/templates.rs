//! Built-in station templates (PLAN §10, M5): one click configures the device and its
//! channels for a known activity, with a short "what am I looking at" explainer.
//!
//! Templates are a static table, not seeded rows: they ship with the binary, so a seeded
//! table would need its own migration every time one is added or corrected, and a user could
//! edit or delete an entry the next release would silently restore. Presets remain the
//! writable, device-bound side of the same idea (PLAN §11); a template is device-*agnostic* —
//! it names a frequency and a mode, and applies to whatever hardware can reach them.

use std::sync::LazyLock;

use sdrmm_wire::{
    AdsbParams, AisParams, AmParams, AprsParams, ChannelParams, ChannelSettings, LayoutNode,
    NfmParams, PanelKind, PocsagParams, SplitDirection, TemplateInfo, WfmParams,
};

/// One channel in a template: its absolute frequency and a constructor for its params.
/// A function rather than a value because `ChannelParams` is not const-constructible.
type Channel = (f64, fn() -> ChannelParams);

/// One channel at an absolute frequency; the offset is resolved against the template's centre
/// when it is applied.
struct Entry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    explainer: &'static str,
    center_hz: f64,
    sample_rate: f64,
    /// `(absolute frequency, params)` — absolute so the table reads like a band plan.
    channels: &'static [Channel],
    /// Panels the template opens as its own workspace tab (M6, PLAN §16). Two shapes cover
    /// every entry: what you *listen* to, and what you *plot*.
    layout: fn() -> LayoutNode,
}

/// Spectrum over the channel controls and whatever the channel decodes — the shape for a
/// template you tune and listen to.
fn listening_layout() -> LayoutNode {
    LayoutNode::split(
        SplitDirection::Column,
        vec![
            (600, LayoutNode::group(&[PanelKind::Spectrum])),
            (
                400,
                LayoutNode::split(
                    SplitDirection::Row,
                    vec![
                        (500, LayoutNode::group(&[PanelKind::Channels])),
                        (500, LayoutNode::group(&[PanelKind::Decoders])),
                    ],
                ),
            ),
        ],
    )
}

/// Spectrum over the decoder views beside the map — the shape for a template whose output has
/// positions (ADS-B, AIS, APRS).
fn mapping_layout() -> LayoutNode {
    LayoutNode::split(
        SplitDirection::Row,
        vec![
            (
                550,
                LayoutNode::split(
                    SplitDirection::Column,
                    vec![
                        (450, LayoutNode::group(&[PanelKind::Spectrum])),
                        (
                            550,
                            LayoutNode::group(&[PanelKind::Decoders, PanelKind::Channels]),
                        ),
                    ],
                ),
            ),
            (450, LayoutNode::group(&[PanelKind::Map])),
        ],
    )
}

/// Every template. Keep the sample rates conservative: the Pi 4 is the performance floor
/// (PLAN §14), and a template is the first thing a new user runs.
static TEMPLATES: &[Entry] = &[
    Entry {
        id: "fm-radio",
        name: "FM radio",
        description: "Wideband FM broadcast with RDS station text.",
        explainer: "The wide humps across the display are FM stations, each about 200 kHz \
                    across. The channel is tuned to one of them; RDS decodes the station name \
                    and radio text from a subcarrier you cannot hear. Drag the channel marker \
                    or edit its offset to move between stations.",
        center_hz: 98_000_000.0,
        sample_rate: 2_400_000.0,
        channels: &[(98_000_000.0, || {
            ChannelParams::Wfm(WfmParams {
                rds: true,
                ..WfmParams::default()
            })
        })],
        layout: listening_layout,
    },
    Entry {
        id: "airband",
        name: "Airband",
        description: "AM aircraft voice — tower, approach and ground.",
        explainer: "Civil aviation voice is AM between 118 and 137 MHz, 25 kHz apart. \
                    Transmissions are short and sporadic: leave the squelch on and wait. \
                    The 2.4 MHz window covers roughly 117–119.3 MHz, so tower, ground and \
                    approach are usually all reachable by moving the channel offset alone. \
                    121.5 MHz is the emergency frequency and is normally silent.",
        center_hz: 118_100_000.0,
        sample_rate: 2_400_000.0,
        channels: &[(118_100_000.0, || ChannelParams::Am(AmParams::default()))],
        layout: listening_layout,
    },
    Entry {
        id: "adsb",
        name: "Aircraft (ADS-B)",
        description: "1090 MHz aircraft positions on the map.",
        explainer: "Aircraft broadcast their identity, altitude, speed and position on \
                    1090 MHz. Reception is line-of-sight, so range depends on antenna height \
                    far more than on gain. The device must run at exactly 2 Msps — ADS-B \
                    fills its whole channel, and a resampled one decodes nothing.",
        // ADS-B occupies its entire 2 MHz channel, so the device rate must equal the channel
        // rate exactly or the engine refuses the channel (PLAN §18, M4 decision).
        center_hz: 1_090_000_000.0,
        sample_rate: 2_000_000.0,
        channels: &[(1_090_000_000.0, || {
            ChannelParams::Adsb(AdsbParams::default())
        })],
        layout: mapping_layout,
    },
    Entry {
        id: "ais",
        name: "Ships (AIS)",
        description: "Marine AIS positions on the map.",
        explainer: "Ships broadcast position and identity on two VHF channels, 161.975 and \
                    162.025 MHz. Both channels are decoded at once. Coverage is line-of-sight \
                    over water, which is usually far better than over land.",
        center_hz: 162_000_000.0,
        sample_rate: 2_400_000.0,
        channels: &[
            (161_975_000.0, || ChannelParams::Ais(AisParams::default())),
            (162_025_000.0, || {
                ChannelParams::Ais(AisParams {
                    ais_channel: sdrmm_wire::AisChannel::B,
                })
            }),
        ],
        layout: mapping_layout,
    },
    Entry {
        id: "aprs",
        name: "APRS",
        description: "Amateur position and message packets (144.800 MHz).",
        explainer: "APRS is AX.25 packet radio at 1200 baud AFSK. Each burst is a short \
                    chirp; stations report position, weather or messages. 144.800 MHz is the \
                    European calling frequency — North America uses 144.390 MHz.",
        center_hz: 144_800_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(144_800_000.0, || ChannelParams::Aprs(AprsParams::default()))],
        layout: mapping_layout,
    },
    Entry {
        id: "pagers",
        name: "Pagers (POCSAG)",
        description: "POCSAG pager messages, 512/1200/2400 baud.",
        explainer: "Pager traffic still carries hospital, industrial and emergency dispatch \
                    messages. The decoder locks onto whichever of the three baud rates a \
                    transmission uses. Frequencies are regional — 466 MHz is common in \
                    Europe; check your national allocation.",
        center_hz: 466_075_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(466_075_000.0, || {
            ChannelParams::Pocsag(PocsagParams::default())
        })],
        layout: listening_layout,
    },
    Entry {
        id: "ham-2m",
        name: "Ham 2 m",
        description: "The 2 metre amateur band, FM repeater output segment.",
        explainer: "145.600–145.800 MHz carries FM repeater outputs, 12.5 kHz apart. Point \
                    the scanner at this range to find the active ones: a repeater is silent \
                    between overs, so a sweep finds far more than sitting on one channel.",
        center_hz: 145_700_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(145_700_000.0, || ChannelParams::Nfm(NfmParams::default()))],
        layout: listening_layout,
    },
    Entry {
        id: "marine-vhf",
        name: "Marine VHF",
        description: "Marine voice channels around 156–162 MHz.",
        explainer: "Channel 16 (156.800 MHz) is the international distress and calling \
                    channel and is monitored everywhere; working traffic moves off it. Marine \
                    VHF is narrowband FM, 25 kHz apart.",
        center_hz: 156_800_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(156_800_000.0, || ChannelParams::Nfm(NfmParams::default()))],
        layout: listening_layout,
    },
];

/// Everything a client can apply, in table order (curated, not alphabetical: the gallery
/// leads with what a first-time user can hear immediately).
#[must_use]
pub(crate) fn all() -> &'static [TemplateInfo] {
    static BUILT: LazyLock<Vec<TemplateInfo>> = LazyLock::new(|| {
        TEMPLATES
            .iter()
            .map(|entry| {
                let channels: Vec<ChannelSettings> = entry
                    .channels
                    .iter()
                    .map(|(freq_hz, params)| ChannelSettings {
                        offset_hz: freq_hz - entry.center_hz,
                        squelch_db: None,
                        params: params(),
                    })
                    .collect();
                let freqs = entry.channels.iter().map(|(f, _)| *f);
                let min = freqs.clone().fold(f64::INFINITY, f64::min);
                let max = freqs.fold(f64::NEG_INFINITY, f64::max);
                TemplateInfo {
                    id: entry.id.to_string(),
                    name: entry.name.to_string(),
                    description: entry.description.to_string(),
                    explainer: entry.explainer.to_string(),
                    center_hz: entry.center_hz,
                    sample_rate: entry.sample_rate,
                    channels,
                    min_freq_hz: min.min(entry.center_hz),
                    max_freq_hz: max.max(entry.center_hz),
                    layout: Some((entry.layout)()),
                }
            })
            .collect()
    });
    &BUILT
}

#[must_use]
pub(crate) fn get(id: &str) -> Option<&'static TemplateInfo> {
    all().iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The id is the apply path segment and the gallery's React key; a duplicate would make
    /// both ambiguous.
    #[test]
    fn ids_are_unique_and_slug_shaped() {
        let mut seen = std::collections::HashSet::new();
        for template in all() {
            assert!(seen.insert(&template.id), "duplicate id {}", template.id);
            assert!(
                template
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "unslugly id {}",
                template.id
            );
            assert!(!template.name.is_empty());
            assert!(!template.explainer.is_empty());
        }
    }

    /// A template that cannot be applied is worse than no template: every channel must sit
    /// inside the passband its own sample rate provides, or the apply fails at the last step
    /// with the set already retuned.
    #[test]
    fn every_channel_fits_its_templates_passband() {
        for template in all() {
            // 40% of the rate, not Nyquist: the outer 20% is the capture filter's transition
            // and the tuner's roll-off, so a channel placed there is quietly degraded rather
            // than rejected — the worst kind of broken template.
            let usable = template.sample_rate * 0.4;
            for channel in &template.channels {
                assert!(
                    channel.offset_hz.abs() < usable,
                    "{}: channel at {} Hz is outside the flat ±{usable} Hz",
                    template.id,
                    channel.offset_hz
                );
            }
            assert!(template.min_freq_hz <= template.max_freq_hz);
        }
    }

    /// ADS-B fills its whole 2 MHz channel, so a resampling DDC cannot deliver it: the
    /// template must name exactly the device rate the engine will accept (PLAN §18).
    #[test]
    fn adsb_template_runs_the_device_at_the_channel_rate() {
        let adsb = get("adsb").expect("adsb template");
        assert_eq!(adsb.sample_rate, 2_000_000.0);
        assert_eq!(adsb.channels.len(), 1);
        assert_eq!(adsb.channels[0].offset_hz, 0.0);
    }

    /// A template's tab is upserted into the live workspace, so a layout that fails validation
    /// would be discovered only when someone clicks Apply.
    #[test]
    fn every_template_layout_is_a_valid_tab() {
        use sdrmm_wire::{TabSpec, WORKSPACE_SNAPSHOT_VERSION, WorkspaceSnapshot};
        for template in all() {
            let layout = template.layout.clone().expect("templates carry a layout");
            let snapshot = WorkspaceSnapshot {
                version: WORKSPACE_SNAPSHOT_VERSION,
                tabs: vec![TabSpec {
                    id: format!("template:{}", template.id),
                    name: template.name.clone(),
                    layout,
                    floating: Vec::new(),
                }],
                active_tab: None,
            };
            snapshot
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", template.id));
        }
    }

    #[test]
    fn unknown_ids_are_none() {
        assert!(get("nope").is_none());
        assert!(get("").is_none());
    }
}

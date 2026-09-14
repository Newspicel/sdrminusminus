use sdrmm_device::DeviceError;
use sdrmm_wire::{
    ArgumentOption, Capabilities, Coherence, DcArtifact, Duplex, ExtraSetting, GainStage, Range,
    StreamScope,
};

use crate::{
    iio::{Client, Context, Direction},
    layout::{
        BB_DC_TRACKING, FILTER_FIR_EN, FREQUENCY, GAIN_CONTROL_MODE, HARDWAREGAIN, Layout,
        NOMINAL_XO_HZ, QUADRATURE_TRACKING, RF_BANDWIDTH, RF_DC_TRACKING, RF_PORT_SELECT, RX_LO,
        SAMPLING_FREQUENCY, XO_CORRECTION, available,
    },
};

pub(crate) const RX_STAGE: &str = "RX";
pub(crate) const TX_STAGE: &str = "TX";

pub(crate) const GAIN_MODE: &str = "gain_mode";
pub(crate) const QUADRATURE: &str = "quadrature_tracking";
pub(crate) const RF_DC: &str = "rf_dc_tracking";
pub(crate) const BB_DC: &str = "bb_dc_tracking";
pub(crate) const FIR: &str = "fir_filter";
pub(crate) const TX_PORT: &str = "tx_port";

/// What the AD936x on this board will accept, as the board itself reports it.
///
/// An AD9361 reaches 6 GHz and an AD9363 stops at 3.8, and the same firmware serves both, so
/// every limit here is read rather than assumed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Front {
    pub(crate) frequency: Range,
    pub(crate) rate: Range,
    pub(crate) rx_bandwidth: Range,
    pub(crate) tx_bandwidth: Option<Range>,
    pub(crate) rx_gain: Range,
    pub(crate) tx_gain: Option<Range>,
    pub(crate) gain_modes: Vec<String>,
    pub(crate) rx_ports: Vec<String>,
    pub(crate) tx_ports: Vec<String>,
    pub(crate) trim: Option<Trim>,
    pub(crate) tracking: Tracking,
}

/// The crystal correction, and the value it was calibrated to, which is what a part per million
/// is counted from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Trim {
    pub(crate) reference: f64,
    pub(crate) range: Range,
}

impl Trim {
    pub(crate) fn correction(&self, ppm: f64) -> f64 {
        (self.reference * (1.0 + ppm / 1e6))
            .round()
            .clamp(self.range.min, self.range.max)
    }

    pub(crate) fn ppm(&self, correction: f64) -> f64 {
        if self.reference <= 0.0 {
            return 0.0;
        }
        (correction / self.reference - 1.0) * 1e6
    }

    pub(crate) fn limit_ppm(&self) -> f64 {
        let low = self.ppm(self.range.min).abs();
        let high = self.ppm(self.range.max).abs();
        low.min(high)
    }
}

/// The corrections the transceiver runs for itself, each present only if this firmware has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tracking {
    pub(crate) quadrature: bool,
    pub(crate) rf_dc: bool,
    pub(crate) bb_dc: bool,
    pub(crate) fir: bool,
}

/// The widest span any AD936x part covers, used only when a firmware publishes no list of its
/// own. The radio still refuses anything its own part cannot reach, and says so.
const FALLBACK_FREQUENCY: Range = span(70e6, 6e9);
const FALLBACK_RATE: Range = span(2_083_333.0, 61_440_000.0);
const FALLBACK_RX_BANDWIDTH: Range = span(200e3, 56e6);
const FALLBACK_TX_BANDWIDTH: Range = span(200e3, 40e6);
const FALLBACK_RX_GAIN: Range = span(-3.0, 71.0);
const FALLBACK_TX_GAIN: Range = Range {
    min: -89.75,
    max: 0.0,
    step: Some(0.25),
};

const fn span(min: f64, max: f64) -> Range {
    Range {
        min,
        max,
        step: None,
    }
}

impl Front {
    pub(crate) fn read(
        client: &Client,
        context: &Context,
        layout: &Layout,
    ) -> Result<Self, DeviceError> {
        let reader = Reader {
            client,
            context,
            phy: &layout.phy,
        };
        let rx = layout.port(false, 0).ok_or_else(|| {
            DeviceError::Unsupported("this radio has no receive port".to_string())
        })?;
        let tx = layout.port(true, 0);
        Ok(Self {
            frequency: continuous(
                reader
                    .range(Direction::Out, RX_LO, FREQUENCY)
                    .unwrap_or(FALLBACK_FREQUENCY),
            ),
            rate: continuous(
                reader
                    .range(Direction::In, rx, SAMPLING_FREQUENCY)
                    .unwrap_or(FALLBACK_RATE),
            ),
            rx_bandwidth: continuous(
                reader
                    .range(Direction::In, rx, RF_BANDWIDTH)
                    .unwrap_or(FALLBACK_RX_BANDWIDTH),
            ),
            tx_bandwidth: tx.map(|tx| {
                continuous(
                    reader
                        .range(Direction::Out, tx, RF_BANDWIDTH)
                        .unwrap_or(FALLBACK_TX_BANDWIDTH),
                )
            }),
            rx_gain: reader
                .range(Direction::In, rx, HARDWAREGAIN)
                .unwrap_or(FALLBACK_RX_GAIN),
            tx_gain: tx.map(|tx| {
                reader
                    .range(Direction::Out, tx, HARDWAREGAIN)
                    .unwrap_or(FALLBACK_TX_GAIN)
            }),
            gain_modes: reader
                .list(Direction::In, rx, GAIN_CONTROL_MODE)
                .unwrap_or_default(),
            rx_ports: reader
                .list(Direction::In, rx, RF_PORT_SELECT)
                .unwrap_or_default(),
            tx_ports: tx
                .and_then(|tx| reader.list(Direction::Out, tx, RF_PORT_SELECT))
                .unwrap_or_default(),
            trim: reader.trim(),
            tracking: reader.tracking(rx),
        })
    }
}

struct Reader<'a> {
    client: &'a Client,
    context: &'a Context,
    phy: &'a str,
}

impl Reader<'_> {
    /// An attribute's value, asked of the radio and falling back to what its own description of
    /// itself already said. A firmware that publishes neither simply does not have it.
    fn value(&self, direction: Direction, channel: &str, attr: &str) -> Option<String> {
        match self
            .client
            .read_channel_attr(self.phy, direction, channel, attr)
        {
            Ok(value) if !value.trim().is_empty() => return Some(value),
            Ok(_) => {}
            Err(e) => tracing::debug!("{}.{channel}.{attr}: {e}", self.phy),
        }
        self.context
            .device(self.phy)?
            .channel(channel, direction == Direction::Out)?
            .attribute(attr)?
            .value
            .clone()
            .filter(|value| value != "ERROR")
    }

    fn range(&self, direction: Direction, channel: &str, attr: &str) -> Option<Range> {
        parse_range(&self.value(direction, channel, &available(attr))?)
    }

    fn list(&self, direction: Direction, channel: &str, attr: &str) -> Option<Vec<String>> {
        let options: Vec<String> = self
            .value(direction, channel, &available(attr))?
            .split_whitespace()
            .map(str::to_string)
            .collect();
        (!options.is_empty()).then_some(options)
    }

    fn present(&self, direction: Direction, channel: &str, attr: &str) -> bool {
        self.value(direction, channel, attr).is_some()
    }

    fn tracking(&self, rx: &str) -> Tracking {
        Tracking {
            quadrature: self.present(Direction::In, rx, QUADRATURE_TRACKING),
            rf_dc: self.present(Direction::In, rx, RF_DC_TRACKING),
            bb_dc: self.present(Direction::In, rx, BB_DC_TRACKING),
            fir: self.present(Direction::In, rx, FILTER_FIR_EN),
        }
    }

    /// The crystal as the board left the factory. Its present correction is the zero of the
    /// operator's parts per million, so a board trimmed at build time stays trimmed at 0 ppm.
    fn trim(&self) -> Option<Trim> {
        let reference = self
            .client
            .read_device_attr(self.phy, XO_CORRECTION)
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .or_else(|| {
                self.context
                    .attribute(&format!("{},{XO_CORRECTION}", self.phy))?
                    .parse()
                    .ok()
            })
            .filter(|reference| *reference > 0.0)?;
        let range = self
            .client
            .read_device_attr(self.phy, &available(XO_CORRECTION))
            .ok()
            .and_then(|value| parse_range(&value))
            .filter(|range| range.max > range.min)
            .unwrap_or(Range {
                min: reference - NOMINAL_XO_HZ * 200.0 / 1e6,
                max: reference + NOMINAL_XO_HZ * 200.0 / 1e6,
                step: None,
            });
        Some(Trim { reference, range })
    }
}

/// `[min step max]`, which is how every bounded IIO attribute publishes its limits.
pub(crate) fn parse_range(text: &str) -> Option<Range> {
    let inside = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut parts = inside.split_whitespace();
    let min: f64 = parts.next()?.parse().ok()?;
    let step: f64 = parts.next()?.parse().ok()?;
    let max: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(min.is_finite() && max.is_finite()) || max < min {
        return None;
    }
    Some(Range {
        min,
        max,
        step: (step > 0.0).then_some(step),
    })
}

/// A one-count step over a range measured in hertz is the absence of a step, not a grid worth
/// snapping a control to.
fn continuous(range: Range) -> Range {
    Range {
        step: range.step.filter(|step| *step > 1.0),
        ..range
    }
}

pub(crate) fn capabilities(front: &Front, layout: &Layout) -> Capabilities {
    let rx_streams = layout.rx_streams() as u32;
    let tx_streams = layout.tx_streams() as u32;
    Capabilities {
        freq_ranges: vec![front.frequency],
        sample_rates: Vec::new(),
        sample_rate_ranges: vec![front.rate],
        gains: gain_stages(front),
        antennas: front.rx_ports.clone(),
        bandwidths: Vec::new(),
        bandwidth_ranges: vec![front.rx_bandwidth],
        extra: extra_settings(front),
        ppm: front.trim.is_some(),
        duplex: if tx_streams > 0 {
            Duplex::Full
        } else {
            Duplex::RxOnly
        },
        rx_streams: rx_streams.max(1),
        tx_streams,
        per_stream: if rx_streams > 1 {
            StreamScope {
                tuning: false,
                gain: true,
                antenna: true,
            }
        } else {
            StreamScope::default()
        },
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        // Both receivers run off the one synthesizer and the one converter clock, so their
        // relative phase survives a retune and a bearing taken across them means something.
        coherence: if rx_streams > 1 {
            Coherence::PhaseCoherent
        } else {
            Coherence::None
        },
        noise_source: false,
    }
}

fn gain_stages(front: &Front) -> Vec<GainStage> {
    let mut stages = vec![GainStage {
        name: RX_STAGE.to_string(),
        range: front.rx_gain,
        values: Vec::new(),
    }];
    if let Some(range) = front.tx_gain {
        stages.push(GainStage {
            name: TX_STAGE.to_string(),
            range,
            values: Vec::new(),
        });
    }
    stages
}

fn extra_settings(front: &Front) -> Vec<ExtraSetting> {
    let mut extra = Vec::new();
    if !front.gain_modes.is_empty() {
        extra.push(ExtraSetting::Enum {
            name: GAIN_MODE.to_string(),
            options: front.gain_modes.iter().map(ArgumentOption::plain).collect(),
            default: front
                .gain_modes
                .first()
                .cloned()
                .unwrap_or_else(|| "manual".to_string()),
        });
    }
    for (present, name) in [
        (front.tracking.quadrature, QUADRATURE),
        (front.tracking.rf_dc, RF_DC),
        (front.tracking.bb_dc, BB_DC),
    ] {
        if present {
            extra.push(ExtraSetting::Bool {
                name: name.to_string(),
                default: true,
            });
        }
    }
    if front.tracking.fir {
        extra.push(ExtraSetting::Bool {
            name: FIR.to_string(),
            default: false,
        });
    }
    if front.tx_ports.len() > 1 {
        extra.push(ExtraSetting::Enum {
            name: TX_PORT.to_string(),
            options: front.tx_ports.iter().map(ArgumentOption::plain).collect(),
            default: front.tx_ports[0].clone(),
        });
    }
    extra
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn front() -> Front {
        Front {
            frequency: span(70e6, 6e9),
            rate: span(2_083_333.0, 61_440_000.0),
            rx_bandwidth: span(200e3, 56e6),
            tx_bandwidth: Some(span(200e3, 40e6)),
            rx_gain: Range {
                min: -3.0,
                max: 71.0,
                step: Some(1.0),
            },
            tx_gain: Some(FALLBACK_TX_GAIN),
            gain_modes: ["manual", "fast_attack", "slow_attack", "hybrid"]
                .map(str::to_string)
                .to_vec(),
            rx_ports: ["A_BALANCED", "B_BALANCED"].map(str::to_string).to_vec(),
            tx_ports: ["A", "B"].map(str::to_string).to_vec(),
            trim: Some(Trim {
                reference: 40_000_000.0,
                range: span(39_992_159.0, 40_008_159.0),
            }),
            tracking: Tracking {
                quadrature: true,
                rf_dc: true,
                bb_dc: true,
                fir: true,
            },
        }
    }

    #[test]
    fn a_bounded_attribute_reads_as_the_range_it_publishes() {
        assert_eq!(
            parse_range("[70000000 1 6000000000]"),
            Some(Range {
                min: 70e6,
                max: 6e9,
                step: Some(1.0)
            })
        );
        assert_eq!(
            parse_range(" [-89.750000 0.250000 0.000000] "),
            Some(Range {
                min: -89.75,
                max: 0.0,
                step: Some(0.25)
            })
        );
        assert_eq!(
            parse_range("[0 0 0]"),
            Some(Range {
                min: 0.0,
                max: 0.0,
                step: None
            }),
            "a disabled trim publishes a zero step"
        );
    }

    #[test]
    fn anything_that_is_not_a_range_is_not_read_as_one() {
        for bad in [
            "",
            "ERROR",
            "manual fast_attack",
            "[1 2]",
            "[1 2 3 4]",
            "[10 1 5]",
            "1 2 3",
        ] {
            assert_eq!(parse_range(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_hertz_range_loses_a_step_of_one_and_keeps_a_real_one() {
        assert_eq!(
            continuous(parse_range("[1 1 9]").expect("range")).step,
            None
        );
        assert_eq!(
            continuous(parse_range("[1 4 9]").expect("range")).step,
            Some(4.0)
        );
    }

    #[test]
    fn a_trim_counts_parts_per_million_from_the_factory_value() {
        let trim = Trim {
            reference: 40_000_000.0,
            range: span(39_992_159.0, 40_008_159.0),
        };
        assert!((trim.correction(0.0) - 40_000_000.0).abs() < 1.0);
        assert!((trim.correction(10.0) - 40_000_400.0).abs() < 1.0);
        assert!((trim.ppm(40_000_400.0) - 10.0).abs() < 0.01);
        assert!(
            (trim.correction(1_000.0) - trim.range.max).abs() < 1.0,
            "a correction past the crystal's reach is clamped to it"
        );
        assert!(trim.limit_ppm() > 100.0 && trim.limit_ppm() < 250.0);
    }

    #[test]
    fn a_two_by_two_radio_declares_per_lane_gain_and_a_shared_synthesizer() {
        let layout = crate::layout::tests::two_by_two_layout();
        let caps = capabilities(&front(), &layout);
        assert_eq!(caps.rx_streams, 2);
        assert_eq!(caps.tx_streams, 2);
        assert_eq!(caps.duplex, Duplex::Full);
        assert_eq!(caps.coherence, Coherence::PhaseCoherent);
        assert!(caps.per_stream.gain);
        assert!(!caps.per_stream.tuning, "one synthesizer feeds both lanes");
        assert_eq!(caps.dc_artifact, DcArtifact::Managed);
        assert!(caps.ppm);
    }

    #[test]
    fn a_receive_only_radio_declares_no_transmitter_and_no_coherence() {
        let layout = crate::layout::tests::one_by_one_layout();
        let caps = capabilities(&front(), &layout);
        assert_eq!(caps.rx_streams, 1);
        assert_eq!(caps.tx_streams, 0);
        assert_eq!(caps.duplex, Duplex::RxOnly);
        assert_eq!(caps.coherence, Coherence::None);
        assert_eq!(caps.per_stream, StreamScope::default());
    }

    #[test]
    fn the_gain_budget_names_a_stage_for_each_direction_that_exists() {
        let caps = capabilities(&front(), &crate::layout::tests::two_by_two_layout());
        let names: Vec<&str> = caps.gains.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec![RX_STAGE, TX_STAGE]);

        let mut receive_only = front();
        receive_only.tx_gain = None;
        let caps = capabilities(&receive_only, &crate::layout::tests::one_by_one_layout());
        assert_eq!(caps.gains.len(), 1);
        assert_eq!(caps.gains[0].range.max, 71.0);
    }

    #[test]
    fn only_the_corrections_this_firmware_carries_become_settings() {
        let names = |front: &Front| {
            extra_settings(front)
                .iter()
                .map(|setting| setting.name().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&front()),
            vec![GAIN_MODE, QUADRATURE, RF_DC, BB_DC, FIR, TX_PORT]
        );

        let bare = Front {
            gain_modes: Vec::new(),
            tx_ports: Vec::new(),
            tracking: Tracking::default(),
            ..front()
        };
        assert!(names(&bare).is_empty());
    }

    #[test]
    fn a_firmware_that_publishes_no_limits_still_reports_a_usable_front_end() {
        let front = Front {
            frequency: FALLBACK_FREQUENCY,
            rate: FALLBACK_RATE,
            rx_bandwidth: FALLBACK_RX_BANDWIDTH,
            ..front()
        };
        let caps = capabilities(&front, &crate::layout::tests::one_by_one_layout());
        assert_eq!(caps.freq_ranges[0].min, 70e6);
        assert_eq!(caps.sample_rate_ranges[0].max, 61_440_000.0);
        assert!(caps.bandwidth_ranges[0].holds(20e6));
    }
}

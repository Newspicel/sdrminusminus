use sdrmm_device::DeviceError;
use sdrmm_wire::{
    Agc, AgcReach, ArgumentOption, Capabilities, Coherence, DcArtifact, Duplex, ExtraSetting,
    GainKind, GainStage, Range, StreamScope,
};

use crate::{
    iio::{Client, Context, Direction},
    layout::{
        BB_DC_TRACKING, CONVERTER_CHANNEL, FREQUENCY, GAIN_CONTROL_MODE, HARDWAREGAIN, Layout,
        QUADRATURE_TRACKING, RF_BANDWIDTH, RF_DC_TRACKING, RF_PORT_SELECT, RX_LO,
        SAMPLING_FREQUENCY, XO_CORRECTION, available,
    },
};

pub(crate) const MANUAL_GAIN: &str = "manual";
const AGC_MODES: [(&str, &str); 3] = [
    ("fast_attack", "Fast attack"),
    ("slow_attack", "Slow attack"),
    ("hybrid", "Hybrid"),
];

pub(crate) const QUADRATURE: &str = "quadrature_tracking";
pub(crate) const RF_DC: &str = "rf_dc_tracking";
pub(crate) const BB_DC: &str = "bb_dc_tracking";
pub(crate) const TX_PORT: &str = "tx_port";
const RX_PORT_LOCK: &str = "adi,rx-rf-port-input-select-lock-enable";
const TX_PORT_LOCK: &str = "adi,tx-rf-port-input-select-lock-enable";

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
    pub(crate) decimation: Option<Decimation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Decimation {
    pub(crate) factor: u32,
    pub(crate) rx: String,
    pub(crate) tx: Option<String>,
}

impl Front {
    pub(crate) fn rates(&self) -> Range {
        match &self.decimation {
            Some(decimation) => Range {
                min: (self.rate.min / f64::from(decimation.factor)).ceil(),
                ..self.rate
            },
            None => self.rate,
        }
    }

    pub(crate) fn converter_rate(&self, rate: f64) -> f64 {
        match &self.decimation {
            Some(decimation) if rate < self.rate.min => rate * f64::from(decimation.factor),
            _ => rate,
        }
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tracking {
    pub(crate) quadrature: bool,
    pub(crate) rf_dc: bool,
    pub(crate) bb_dc: bool,
}

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
            rx_ports: reader.ports(Direction::In, rx, RX_PORT_LOCK),
            tx_ports: tx
                .map(|tx| reader.ports(Direction::Out, tx, TX_PORT_LOCK))
                .unwrap_or_default(),
            trim: reader.trim(),
            tracking: reader.tracking(rx),
            decimation: decimation(client, context, layout),
        })
    }
}

struct Reader<'a> {
    client: &'a Client,
    context: &'a Context,
    phy: &'a str,
}

impl Reader<'_> {
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

    fn ports(&self, direction: Direction, channel: &str, lock: &str) -> Vec<String> {
        if self.locked(lock) {
            return self
                .value(direction, channel, RF_PORT_SELECT)
                .map(|port| vec![port.trim().to_string()])
                .unwrap_or_default();
        }
        self.list(direction, channel, RF_PORT_SELECT)
            .unwrap_or_default()
    }

    fn locked(&self, lock: &str) -> bool {
        self.client
            .read_debug_attr(self.phy, lock)
            .inspect_err(|e| tracing::debug!("{}.{lock}: {e}", self.phy))
            .is_ok_and(|value| value.trim() == "1")
    }

    fn present(&self, rx: &str, attr: &str) -> bool {
        self.context
            .device(self.phy)
            .and_then(|device| device.channel(rx, false))
            .is_some_and(|channel| channel.has(attr))
    }

    fn tracking(&self, rx: &str) -> Tracking {
        Tracking {
            quadrature: self.present(rx, QUADRATURE_TRACKING),
            rf_dc: self.present(rx, RF_DC_TRACKING),
            bb_dc: self.present(rx, BB_DC_TRACKING),
        }
    }

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
                min: reference - reference * 200.0 / 1e6,
                max: reference + reference * 200.0 / 1e6,
                step: None,
            });
        Some(Trim { reference, range })
    }
}

fn decimation(client: &Client, context: &Context, layout: &Layout) -> Option<Decimation> {
    let rx = layout.rx.as_ref()?.device.clone();
    let offered = client
        .read_channel_attr(
            &rx,
            Direction::In,
            CONVERTER_CHANNEL,
            &available(SAMPLING_FREQUENCY),
        )
        .inspect_err(|e| tracing::debug!("{rx} decimation: {e}"))
        .ok()?;
    let factor = decimation_factor(&offered)?;
    let tx = layout.tx.as_ref().map(|tx| tx.device.clone()).filter(|tx| {
        context
            .device(tx)
            .and_then(|device| device.channel(CONVERTER_CHANNEL, true))
            .is_some_and(|channel| channel.has(SAMPLING_FREQUENCY))
    });
    Some(Decimation { factor, rx, tx })
}

pub(crate) fn decimation_factor(offered: &str) -> Option<u32> {
    let rates: Vec<f64> = offered
        .split_whitespace()
        .filter_map(|rate| rate.parse().ok())
        .filter(|rate: &f64| *rate > 0.0)
        .collect();
    let [full, reduced] = rates[..] else {
        return None;
    };
    let factor = (full.max(reduced) / full.min(reduced)).round();
    (2.0..=64.0).contains(&factor).then_some(factor as u32)
}

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

fn continuous(range: Range) -> Range {
    Range {
        step: range.step.filter(|step| *step > 1.0),
        ..range
    }
}

pub(crate) fn capabilities(front: &Front, layout: &Layout) -> Capabilities {
    let rx_streams = layout.rx_streams() as u32;
    let tx_streams = layout.tx_streams() as u32;
    let mut capabilities = Capabilities {
        freq_ranges: vec![front.frequency],
        sample_rates: Vec::new(),
        sample_rate_ranges: vec![front.rates()],
        gains: gain_stages(front),
        antennas: front.rx_ports.clone(),
        bandwidths: Vec::new(),
        bandwidth_ranges: vec![front.rx_bandwidth],
        bandwidth_auto: false,
        bias_tee: false,
        agc: agc(front),
        extra: extra_settings(front),
        ppm: front.trim.is_some(),
        duplex: if tx_streams > 0 {
            Duplex::Full
        } else {
            Duplex::RxOnly
        },
        rx_streams: 1,
        tx_streams,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Managed,
        hardware_sweep: false,
        coherence: Coherence::None,
        noise_source: sdrmm_wire::NoiseSource::None,
        retune_keeps_phase: false,
        rx_stream_choices: if rx_streams > 1 {
            (1..=rx_streams).collect()
        } else {
            Vec::new()
        },
    };
    stream_lanes(&mut capabilities, 1, tx_streams);
    capabilities
}

pub(crate) fn stream_lanes(capabilities: &mut Capabilities, lanes: u32, transmitters: u32) {
    capabilities.rx_streams = lanes.max(1);
    capabilities.tx_streams = transmitters.min(capabilities.rx_streams);
    if lanes > 1 {
        capabilities.per_stream = StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
            agc: true,
        };
        capabilities.coherence = Coherence::PhaseCoherent;
    } else {
        capabilities.per_stream = StreamScope::default();
        capabilities.coherence = Coherence::None;
    }
}

fn gain_stages(front: &Front) -> Vec<GainStage> {
    let mut stages = vec![GainStage::new(GainKind::Tuner, front.rx_gain)];
    if let Some(range) = front.tx_gain {
        stages.push(GainStage::new(GainKind::Tx, range).with_agc(AgcReach::Never));
    }
    stages
}

fn agc(front: &Front) -> Agc {
    let options: Vec<ArgumentOption> = AGC_MODES
        .iter()
        .filter(|(mode, _)| front.gain_modes.iter().any(|offered| offered == mode))
        .map(|(mode, label)| ArgumentOption {
            value: (*mode).to_string(),
            label: Some((*label).to_string()),
        })
        .collect();
    if options.is_empty() {
        Agc::None
    } else {
        Agc::Modes { options }
    }
}

fn extra_settings(front: &Front) -> Vec<ExtraSetting> {
    let mut extra = Vec::new();
    for (present, name, label) in [
        (front.tracking.quadrature, QUADRATURE, "Quadrature tracking"),
        (front.tracking.rf_dc, RF_DC, "RF DC tracking"),
        (front.tracking.bb_dc, BB_DC, "Baseband DC tracking"),
    ] {
        if present {
            extra.push(ExtraSetting::bool(name, label, true));
        }
    }
    if front.tx_ports.len() > 1 {
        extra.push(ExtraSetting::choice(
            TX_PORT,
            "TX port",
            front.tx_ports.iter().map(ArgumentOption::plain).collect(),
            front.tx_ports[0].clone(),
        ));
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
            },
            decimation: Some(Decimation {
                factor: 8,
                rx: "cf-ad9361-lpc".to_string(),
                tx: Some("cf-ad9361-dds-core-lpc".to_string()),
            }),
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
        let mut caps = capabilities(&front(), &layout);
        stream_lanes(&mut caps, 2, 2);
        assert_eq!(caps.rx_streams, 2);
        assert_eq!(caps.tx_streams, 2);
        assert_eq!(caps.duplex, Duplex::Full);
        assert_eq!(caps.coherence, Coherence::PhaseCoherent);
        assert!(caps.per_stream.gain);
        assert!(caps.per_stream.agc, "each receiver runs its own gain loop");
        assert!(
            !caps.per_stream.antenna,
            "one input switch serves both receivers"
        );
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
        let kinds: Vec<GainKind> = caps.gains.iter().map(|g| g.kind).collect();
        assert_eq!(kinds, vec![GainKind::Tuner, GainKind::Tx]);
        assert_eq!(caps.gains[0].name, "TUNER");
        assert_eq!(caps.gains[1].name, "TX");
        assert_eq!(caps.gains[0].agc, AgcReach::Always);
        assert_eq!(
            caps.gains[1].agc,
            AgcReach::Never,
            "receive AGC leaves TX alone"
        );

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
        assert_eq!(names(&front()), vec![QUADRATURE, RF_DC, BB_DC, TX_PORT]);
        assert!(
            extra_settings(&front())
                .iter()
                .all(|setting| setting.label().is_some())
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
    fn the_automatic_gain_modes_are_the_ones_this_firmware_offers() {
        let Agc::Modes { options } = agc(&front()) else {
            panic!("a front end with attack modes offers them");
        };
        let modes: Vec<&str> = options.iter().map(|option| option.value.as_str()).collect();
        assert_eq!(modes, vec!["fast_attack", "slow_attack", "hybrid"]);
        assert!(options.iter().all(|option| option.label.is_some()));

        let slow_only = Front {
            gain_modes: ["manual", "slow_attack"].map(str::to_string).to_vec(),
            ..front()
        };
        assert_eq!(agc(&slow_only).first_mode(), Some("slow_attack"));

        let manual_only = Front {
            gain_modes: vec!["manual".to_string()],
            ..front()
        };
        assert_eq!(agc(&manual_only), Agc::None);
        assert_eq!(
            agc(&Front {
                gain_modes: Vec::new(),
                ..front()
            }),
            Agc::None
        );
    }

    #[test]
    fn a_firmware_that_publishes_no_limits_still_reports_a_usable_front_end() {
        let front = Front {
            frequency: FALLBACK_FREQUENCY,
            rate: FALLBACK_RATE,
            rx_bandwidth: FALLBACK_RX_BANDWIDTH,
            decimation: None,
            ..front()
        };
        let caps = capabilities(&front, &crate::layout::tests::one_by_one_layout());
        assert_eq!(caps.freq_ranges[0].min, 70e6);
        assert_eq!(caps.sample_rate_ranges[0].max, 61_440_000.0);
        assert!(caps.bandwidth_ranges[0].holds(20e6));
    }

    #[test]
    fn a_decimator_reaches_below_the_transceivers_floor() {
        let front = front();
        assert_eq!(front.rates().min, 260_417.0);
        assert_eq!(front.rates().max, 61_440_000.0);
        assert_eq!(front.converter_rate(250_000.0 * 2.0), 4_000_000.0);
        assert_eq!(
            front.converter_rate(2_400_000.0),
            2_400_000.0,
            "a rate the transceiver makes is not decimated"
        );
        let caps = capabilities(&front, &crate::layout::tests::two_by_two_layout());
        assert_eq!(caps.sample_rate_ranges[0].min, 260_417.0);
    }

    #[test]
    fn the_decimator_is_the_ratio_of_the_two_rates_offered() {
        assert_eq!(decimation_factor("30720000 3840000 "), Some(8));
        assert_eq!(decimation_factor("2399999 299999"), Some(8));
        assert_eq!(decimation_factor("30720000"), None);
        assert_eq!(decimation_factor("30720000 30720000"), None);
        assert_eq!(decimation_factor(""), None);
        assert_eq!(decimation_factor("[2083333 1 61440000]"), None);
    }

    #[test]
    fn a_two_by_two_radio_opens_on_one_lane_and_transmits_on_as_many() {
        let mut caps = capabilities(&front(), &crate::layout::tests::two_by_two_layout());
        assert_eq!(caps.rx_stream_choices, vec![1, 2]);
        assert_eq!(caps.rx_streams, 1, "the lanes share one synthesizer");
        assert_eq!(caps.tx_streams, 1);
        assert_eq!(caps.per_stream, StreamScope::default());
        assert_eq!(caps.coherence, Coherence::None);
        stream_lanes(&mut caps, 2, 2);
        assert_eq!(caps.tx_streams, 2);
        assert!(caps.per_stream.agc);
        assert_eq!(caps.coherence, Coherence::PhaseCoherent);

        let single = capabilities(&front(), &crate::layout::tests::one_by_one_layout());
        assert!(single.rx_stream_choices.is_empty());
    }
}

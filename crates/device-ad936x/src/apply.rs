use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    AgcSetting, BandwidthSetting, Capabilities, DeviceSettings, ExtraSetting, ExtraValue, GainKind,
    GainStage, GainValue, StreamSettings,
};

use crate::{
    caps::{BB_DC, Front, MANUAL_GAIN, QUADRATURE, RF_DC, TX_PORT},
    iio::{Client, Direction},
    layout::{
        BB_DC_TRACKING, CONVERTER_CHANNEL, FREQUENCY, GAIN_CONTROL_MODE, HARDWAREGAIN, Layout,
        QUADRATURE_TRACKING, RF_BANDWIDTH, RF_DC_TRACKING, RF_PORT_SELECT, RX_LO,
        SAMPLING_FREQUENCY, TX_LO, XO_CORRECTION,
    },
};

/// One attribute write, in the order the transceiver needs them: a rate change re-derives the
/// filters a bandwidth sits in, and a retune recalibrates against both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    Device {
        attr: String,
        value: String,
    },
    Channel {
        output: bool,
        channel: String,
        attr: String,
        value: String,
    },
    Converter {
        device: String,
        output: bool,
        attr: String,
        value: String,
    },
}

const fn direction(output: bool) -> Direction {
    if output {
        Direction::Out
    } else {
        Direction::In
    }
}

impl Write {
    fn channel(output: bool, channel: &str, attr: &str, value: String) -> Self {
        Self::Channel {
            output,
            channel: channel.to_string(),
            attr: attr.to_string(),
            value,
        }
    }
}

pub(crate) fn execute(client: &Client, phy: &str, writes: &[Write]) -> Result<(), DeviceError> {
    for write in writes {
        match write {
            Write::Device { attr, value } => client.write_device_attr(phy, attr, value)?,
            Write::Channel {
                output,
                channel,
                attr,
                value,
            } => client.write_channel_attr(phy, direction(*output), channel, attr, value)?,
            Write::Converter {
                device,
                output,
                attr,
                value,
            } => client.write_channel_attr(
                device,
                direction(*output),
                CONVERTER_CHANNEL,
                attr,
                value,
            )?,
        }
    }
    Ok(())
}

/// Turns a settings delta into the attribute writes that realise it, refusing anything this
/// board cannot hold rather than letting the radio reinterpret it silently.
pub(crate) fn plan(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    front: &Front,
    layout: &Layout,
    current: &DeviceSettings,
) -> Result<(DeviceSettings, Vec<Write>), DeviceError> {
    check_stream_settings(delta, capabilities)?;
    let mut writes = Vec::new();
    let mut next = current.clone();
    next.merge_from(delta);

    plan_rate(delta, capabilities, front, layout, &mut writes)?;
    plan_bandwidth(delta, capabilities, front, layout, &mut writes)?;
    plan_tuning(delta, capabilities, layout, &mut writes)?;
    plan_trim(delta, front, &mut writes)?;
    plan_lanes(delta, capabilities, layout, &mut writes)?;
    plan_extra(delta, capabilities, layout, &mut writes)?;
    settle_lanes(&mut next, delta);
    next.gains = snapped(&next.gains, capabilities);
    for stream in &mut next.streams {
        stream.gains = snapped(&stream.gains, capabilities);
    }
    Ok((next, writes))
}

/// A top-level value reached every lane, so a lane's own earlier value for it is gone unless
/// this same delta set the lane apart again.
fn settle_lanes(next: &mut DeviceSettings, delta: &DeviceSettings) {
    for stream in &mut next.streams {
        let own = delta.streams.iter().find(|s| s.stream == stream.stream);
        if delta.antenna.is_some() && own.is_none_or(|s| s.antenna.is_none()) {
            stream.antenna = None;
        }
        if delta.agc.is_some() && own.is_none_or(|s| s.agc.is_none()) {
            stream.agc = None;
        }
        for gain in &delta.gains {
            if own.is_none_or(|s| s.gains.iter().all(|g| g.stage != gain.stage)) {
                stream.gains.retain(|g| g.stage != gain.stage);
            }
        }
    }
}

/// The top-level gain and antenna are every lane's, and a `streams` entry is one lane's own on
/// top of that, which is the contract `DeviceSettings::for_stream` reads them by.
fn plan_lanes(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    plan_gains(&delta.gains, capabilities, layout, None, writes)?;
    plan_antenna(delta.antenna.as_deref(), capabilities, layout, writes)?;
    plan_agc(delta.agc.as_ref(), capabilities, layout, None, writes)?;
    for stream in &delta.streams {
        let lane = Some(stream.stream as usize);
        plan_gains(&stream.gains, capabilities, layout, lane, writes)?;
        plan_agc(stream.agc.as_ref(), capabilities, layout, lane, writes)?;
    }
    Ok(())
}

/// The lanes a setting reaches: the one named, or every one this direction has.
fn lanes(layout: &Layout, output: bool, lane: Option<usize>) -> std::ops::Range<usize> {
    match lane {
        Some(lane) => lane..lane + 1,
        None => 0..layout.ports(output).len(),
    }
}

fn plan_rate(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    front: &Front,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let Some(rate) = delta.sample_rate else {
        return Ok(());
    };
    if !sdrmm_wire::any_range_holds(&capabilities.sample_rate_ranges, rate) {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate} Hz is outside what this radio converts"
        )));
    }
    let port = rx_port(layout, 0)?;
    writes.push(Write::channel(
        false,
        port,
        SAMPLING_FREQUENCY,
        whole(front.converter_rate(rate)),
    ));
    if let Some(decimation) = &front.decimation {
        let converters = std::iter::once((false, &decimation.rx))
            .chain(decimation.tx.iter().map(|tx| (true, tx)));
        for (output, device) in converters {
            writes.push(Write::Converter {
                device: device.clone(),
                output,
                attr: SAMPLING_FREQUENCY.to_string(),
                value: whole(rate),
            });
        }
    }
    Ok(())
}

fn plan_bandwidth(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    front: &Front,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let hz = match delta.bandwidth {
        None => return Ok(()),
        Some(BandwidthSetting::Auto) => {
            return Err(DeviceError::Unsupported(
                "bandwidth: this radio does not pick its own filter width".to_string(),
            ));
        }
        Some(BandwidthSetting::Manual { hz }) => hz,
    };
    if !sdrmm_wire::any_range_holds(&capabilities.bandwidth_ranges, hz) {
        return Err(DeviceError::Unsupported(format!(
            "bandwidth {hz} Hz is outside this radio's analog filter"
        )));
    }
    writes.push(Write::channel(
        false,
        rx_port(layout, 0)?,
        RF_BANDWIDTH,
        whole(hz),
    ));
    // The transmit filter has a narrower reach than the receive one, so a width the receiver
    // takes is clamped rather than refused for a direction the operator did not ask about.
    if let (Some(port), Some(range)) = (layout.port(true, 0), front.tx_bandwidth) {
        writes.push(Write::channel(
            true,
            port,
            RF_BANDWIDTH,
            whole(hz.clamp(range.min, range.max)),
        ));
    }
    Ok(())
}

fn plan_tuning(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let Some(hz) = delta.center_hz else {
        return Ok(());
    };
    if !sdrmm_wire::any_range_holds(&capabilities.freq_ranges, hz) {
        return Err(DeviceError::Unsupported(format!(
            "center_hz {hz} is outside this radio's tuning range"
        )));
    }
    writes.push(Write::channel(true, RX_LO, FREQUENCY, whole(hz)));
    // The transmit synthesizer is a separate one on this part. It follows the dial so that a
    // transmission lands where the operator tuned rather than wherever it was last left.
    if layout.tx_streams() > 0 {
        writes.push(Write::channel(true, TX_LO, FREQUENCY, whole(hz)));
    }
    Ok(())
}

fn plan_trim(
    delta: &DeviceSettings,
    front: &Front,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let Some(ppm) = delta.ppm else {
        return Ok(());
    };
    let Some(trim) = front.trim else {
        return Err(DeviceError::Unsupported(
            "this radio's crystal cannot be trimmed".to_string(),
        ));
    };
    let limit = trim.limit_ppm();
    if !ppm.is_finite() || ppm.abs() > limit {
        return Err(DeviceError::Unsupported(format!(
            "ppm {ppm} is outside the ±{limit:.0} this crystal can be pulled"
        )));
    }
    writes.push(Write::Device {
        attr: XO_CORRECTION.to_string(),
        value: whole(trim.correction(ppm)),
    });
    Ok(())
}

fn plan_gains(
    gains: &[GainValue],
    capabilities: &Capabilities,
    layout: &Layout,
    lane: Option<usize>,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    for gain in gains {
        let stage = capabilities
            .gains
            .iter()
            .find(|stage| stage.name == gain.stage)
            .ok_or_else(|| {
                DeviceError::Unsupported(format!("this radio has no {} gain stage", gain.stage))
            })?;
        let output = stage.kind == GainKind::Tx;
        let reached = lanes(layout, output, lane);
        if reached.is_empty() {
            return Err(DeviceError::Unsupported(format!(
                "this radio has no {} lane to set",
                gain.stage
            )));
        }
        for lane in reached {
            let port = layout.port(output, lane).ok_or_else(|| {
                DeviceError::Unsupported(format!("this radio has no {} lane {lane}", gain.stage))
            })?;
            writes.push(Write::channel(
                output,
                port,
                HARDWAREGAIN,
                decibels(stage.snap(gain.value_db)),
            ));
        }
    }
    Ok(())
}

fn plan_antenna(
    antenna: Option<&str>,
    capabilities: &Capabilities,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let Some(antenna) = antenna else {
        return Ok(());
    };
    if !capabilities.antennas.iter().any(|port| port == antenna) {
        return Err(DeviceError::Unsupported(format!(
            "this radio has no {antenna} input; it has {}",
            capabilities.antennas.join(", ")
        )));
    }
    writes.push(Write::channel(
        false,
        rx_port(layout, 0)?,
        RF_PORT_SELECT,
        antenna.to_string(),
    ));
    Ok(())
}

fn plan_agc(
    agc: Option<&AgcSetting>,
    capabilities: &Capabilities,
    layout: &Layout,
    lane: Option<usize>,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    let Some(agc) = agc else {
        return Ok(());
    };
    let mode = gain_control_mode(agc, capabilities)?;
    for lane in lanes(layout, false, lane) {
        writes.push(Write::channel(
            false,
            rx_port(layout, lane)?,
            GAIN_CONTROL_MODE,
            mode.to_string(),
        ));
    }
    Ok(())
}

fn gain_control_mode<'a>(
    agc: &'a AgcSetting,
    capabilities: &'a Capabilities,
) -> Result<&'a str, DeviceError> {
    if !capabilities.agc.admits(agc) {
        return Err(DeviceError::Unsupported(match &agc.mode {
            Some(mode) => format!("agc: this radio has no {mode} mode"),
            None => "agc: this radio has no automatic gain".to_string(),
        }));
    }
    if !agc.on {
        return Ok(MANUAL_GAIN);
    }
    agc.mode
        .as_deref()
        .or_else(|| capabilities.agc.first_mode())
        .ok_or_else(|| {
            DeviceError::Unsupported("agc: this radio has no automatic gain".to_string())
        })
}

fn plan_extra(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    layout: &Layout,
    writes: &mut Vec<Write>,
) -> Result<(), DeviceError> {
    for extra in &delta.extra {
        let declared = capabilities
            .extra
            .iter()
            .find(|setting| setting.name() == extra.name)
            .ok_or_else(|| {
                DeviceError::Unsupported(format!("this radio has no {} setting", extra.name))
            })?;
        match extra.name.as_str() {
            TX_PORT => {
                let port = choice(extra, declared)?;
                let channel = layout.port(true, 0).ok_or_else(|| {
                    DeviceError::Unsupported("this radio does not transmit".to_string())
                })?;
                writes.push(Write::channel(true, channel, RF_PORT_SELECT, port));
            }
            QUADRATURE | RF_DC | BB_DC => {
                writes.push(Write::channel(
                    false,
                    rx_port(layout, 0)?,
                    tracking_attr(&extra.name),
                    u8::from(flag(extra)?).to_string(),
                ));
            }
            other => {
                return Err(DeviceError::Unsupported(format!(
                    "this radio has no {other} setting"
                )));
            }
        }
    }
    Ok(())
}

fn tracking_attr(name: &str) -> &'static str {
    match name {
        QUADRATURE => QUADRATURE_TRACKING,
        RF_DC => RF_DC_TRACKING,
        _ => BB_DC_TRACKING,
    }
}

fn choice(extra: &ExtraValue, declared: &ExtraSetting) -> Result<String, DeviceError> {
    let ExtraSetting::Enum { options, .. } = declared else {
        return Err(DeviceError::Unsupported(format!(
            "{} is not a choice on this radio",
            extra.name
        )));
    };
    let wanted = extra.value.as_str().ok_or_else(|| {
        DeviceError::Unsupported(format!("{} takes one of the listed settings", extra.name))
    })?;
    if !options.iter().any(|option| option.value == wanted) {
        return Err(DeviceError::Unsupported(format!(
            "{} has no setting {wanted}; it has {}",
            extra.name,
            options
                .iter()
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(wanted.to_string())
}

fn flag(extra: &ExtraValue) -> Result<bool, DeviceError> {
    extra
        .value
        .as_bool()
        .ok_or_else(|| DeviceError::Unsupported(format!("{} is on or off", extra.name)))
}

fn rx_port(layout: &Layout, lane: usize) -> Result<&str, DeviceError> {
    layout
        .port(false, lane)
        .ok_or_else(|| DeviceError::Unsupported(format!("this radio has no receive lane {lane}")))
}

fn snapped(gains: &[GainValue], capabilities: &Capabilities) -> Vec<GainValue> {
    gains
        .iter()
        .map(|gain| match stage(capabilities, &gain.stage) {
            Some(stage) => GainValue {
                stage: gain.stage.clone(),
                value_db: stage.snap(gain.value_db),
            },
            None => gain.clone(),
        })
        .collect()
}

fn stage<'a>(capabilities: &'a Capabilities, name: &str) -> Option<&'a GainStage> {
    capabilities.gains.iter().find(|stage| stage.name == name)
}

/// Hertz attributes are whole numbers; anything else is refused by the driver on the radio.
fn whole(value: f64) -> String {
    format!("{:.0}", value.round())
}

fn decibels(value: f64) -> String {
    format!("{value:.6}")
}

/// Reads back what the radio is set to, so that a freshly opened device reports its own state
/// rather than a guess the operator then has to correct.
pub(crate) fn read_settings(
    client: &Client,
    capabilities: &Capabilities,
    front: &Front,
    layout: &Layout,
) -> DeviceSettings {
    let phy = layout.phy.as_str();
    let rx = layout.port(false, 0);
    let read = |direction, channel: &str, attr: &str| {
        client
            .read_channel_attr(phy, direction, channel, attr)
            .inspect_err(|e| tracing::debug!("{phy}.{channel}.{attr}: {e}"))
            .ok()
    };
    DeviceSettings {
        center_hz: read(Direction::Out, RX_LO, FREQUENCY).and_then(|v| number(&v)),
        sample_rate: read_rate(client, front, rx, &read),
        bandwidth: rx
            .and_then(|rx| read(Direction::In, rx, RF_BANDWIDTH))
            .and_then(|v| number(&v))
            .map(|hz| BandwidthSetting::Manual { hz }),
        antenna: rx.and_then(|rx| read(Direction::In, rx, RF_PORT_SELECT)),
        ppm: read_ppm(client, front, phy),
        agc: read_agc(capabilities, layout, 0, &read),
        gains: read_lane_gains(capabilities, layout, 0, &read),
        extra: read_extra(capabilities, layout, &read),
        streams: read_streams(capabilities, layout, &read),
        ..DeviceSettings::default()
    }
}

fn read_rate(
    client: &Client,
    front: &Front,
    rx: Option<&str>,
    read: &dyn Fn(Direction, &str, &str) -> Option<String>,
) -> Option<f64> {
    let converted = front.decimation.as_ref().and_then(|decimation| {
        client
            .read_channel_attr(
                &decimation.rx,
                Direction::In,
                CONVERTER_CHANNEL,
                SAMPLING_FREQUENCY,
            )
            .inspect_err(|e| tracing::debug!("{}.{SAMPLING_FREQUENCY}: {e}", decimation.rx))
            .ok()
    });
    converted
        .or_else(|| read(Direction::In, rx?, SAMPLING_FREQUENCY))
        .and_then(|v| number(&v))
}

/// What every lane past the first holds of its own, so a two-lane board whose lanes were set
/// apart comes back that way rather than as two copies of lane 0.
fn read_streams(
    capabilities: &Capabilities,
    layout: &Layout,
    read: &dyn Fn(Direction, &str, &str) -> Option<String>,
) -> Vec<StreamSettings> {
    if !capabilities.per_stream.gain && !capabilities.per_stream.agc {
        return Vec::new();
    }
    (1..capabilities.rx_streams as usize)
        .map(|lane| StreamSettings {
            stream: lane as u32,
            gains: read_lane_gains(capabilities, layout, lane, read),
            agc: read_agc(capabilities, layout, lane, read),
            ..StreamSettings::default()
        })
        .collect()
}

fn read_agc(
    capabilities: &Capabilities,
    layout: &Layout,
    lane: usize,
    read: &dyn Fn(Direction, &str, &str) -> Option<String>,
) -> Option<AgcSetting> {
    if !capabilities.agc.offered() {
        return None;
    }
    let mode = read(Direction::In, layout.port(false, lane)?, GAIN_CONTROL_MODE)?;
    let mode = mode.trim();
    if mode == MANUAL_GAIN {
        Some(AgcSetting::off())
    } else {
        Some(AgcSetting::in_mode(true, mode))
    }
}

fn read_ppm(client: &Client, front: &Front, phy: &str) -> Option<f64> {
    let trim = front.trim?;
    let correction = client
        .read_device_attr(phy, XO_CORRECTION)
        .ok()
        .and_then(|value| number(&value))?;
    Some(trim.ppm(correction))
}

pub(crate) fn read_lane_gains(
    capabilities: &Capabilities,
    layout: &Layout,
    lane: usize,
    read: &dyn Fn(Direction, &str, &str) -> Option<String>,
) -> Vec<GainValue> {
    capabilities
        .gains
        .iter()
        .filter_map(|stage| {
            let output = stage.kind == GainKind::Tx;
            let port = layout.port(output, lane)?;
            Some(GainValue {
                stage: stage.name.clone(),
                value_db: number(&read(direction(output), port, HARDWAREGAIN)?)?,
            })
        })
        .collect()
}

fn read_extra(
    capabilities: &Capabilities,
    layout: &Layout,
    read: &dyn Fn(Direction, &str, &str) -> Option<String>,
) -> Vec<ExtraValue> {
    let rx = layout.port(false, 0);
    capabilities
        .extra
        .iter()
        .filter_map(|setting| {
            let name = setting.name();
            let value = match name {
                TX_PORT => {
                    serde_value(read(Direction::Out, layout.port(true, 0)?, RF_PORT_SELECT)?)
                }
                QUADRATURE | RF_DC | BB_DC => {
                    let raw = read(Direction::In, rx?, tracking_attr(name))?;
                    serde_json::Value::Bool(number(&raw)? != 0.0)
                }
                _ => return None,
            };
            Some(ExtraValue {
                name: name.to_string(),
                value,
            })
        })
        .collect()
}

fn serde_value(text: String) -> serde_json::Value {
    serde_json::Value::String(text)
}

/// The leading number of an attribute value. Gains read back as `71.000000 dB`, and the unit is
/// the radio describing itself rather than part of the setting.
fn number(text: &str) -> Option<f64> {
    text.split_whitespace()
        .next()?
        .parse()
        .ok()
        .filter(|value: &f64| value.is_finite())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::caps::{capabilities, tests::front};

    const RX_STAGE: &str = "TUNER";
    const TX_STAGE: &str = "TX";

    fn planned(delta: DeviceSettings) -> Vec<Write> {
        let layout = crate::layout::tests::two_by_two_layout();
        let front = front();
        let capabilities = capabilities(&front, &layout);
        plan(
            &delta,
            &capabilities,
            &front,
            &layout,
            &DeviceSettings::default(),
        )
        .expect("planned")
        .1
    }

    fn refused(delta: DeviceSettings) -> String {
        let layout = crate::layout::tests::two_by_two_layout();
        let front = front();
        let capabilities = capabilities(&front, &layout);
        plan(
            &delta,
            &capabilities,
            &front,
            &layout,
            &DeviceSettings::default(),
        )
        .expect_err("refused")
        .to_string()
    }

    fn channel(output: bool, channel: &str, attr: &str, value: &str) -> Write {
        Write::channel(output, channel, attr, value.to_string())
    }

    fn converter(output: bool, device: &str, value: &str) -> Write {
        Write::Converter {
            device: device.to_string(),
            output,
            attr: SAMPLING_FREQUENCY.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn a_rate_below_the_transceivers_floor_runs_it_faster_and_decimates() {
        let writes = planned(DeviceSettings {
            sample_rate: Some(500_000.0),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", SAMPLING_FREQUENCY, "4000000"),
                converter(false, "cf-ad9361-lpc", "500000"),
                converter(true, "cf-ad9361-dds-core-lpc", "500000"),
            ]
        );
        assert!(
            refused(DeviceSettings {
                sample_rate: Some(200_000.0),
                ..DeviceSettings::default()
            })
            .contains("converts")
        );
    }

    #[test]
    fn the_rate_is_written_before_the_filter_and_the_filter_before_the_dial() {
        let writes = planned(DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(2_400_000.0),
            bandwidth: Some(BandwidthSetting::Manual { hz: 2_000_000.0 }),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", SAMPLING_FREQUENCY, "2400000"),
                converter(false, "cf-ad9361-lpc", "2400000"),
                converter(true, "cf-ad9361-dds-core-lpc", "2400000"),
                channel(false, "voltage0", RF_BANDWIDTH, "2000000"),
                channel(true, "voltage0", RF_BANDWIDTH, "2000000"),
                channel(true, RX_LO, FREQUENCY, "433920000"),
                channel(true, TX_LO, FREQUENCY, "433920000"),
            ]
        );
    }

    #[test]
    fn a_width_the_transmitter_cannot_hold_is_clamped_for_it_alone() {
        let writes = planned(DeviceSettings {
            bandwidth: Some(BandwidthSetting::Manual { hz: 50_000_000.0 }),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", RF_BANDWIDTH, "50000000"),
                channel(true, "voltage0", RF_BANDWIDTH, "40000000"),
            ]
        );
    }

    #[test]
    fn each_gain_stage_reaches_the_channel_of_its_own_direction() {
        let writes = planned(DeviceSettings {
            gains: vec![
                GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 40.4,
                },
                GainValue {
                    stage: TX_STAGE.to_string(),
                    value_db: -10.1,
                },
            ],
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", HARDWAREGAIN, "40.000000"),
                channel(false, "voltage1", HARDWAREGAIN, "40.000000"),
                channel(true, "voltage0", HARDWAREGAIN, "-10.000000"),
                channel(true, "voltage1", HARDWAREGAIN, "-10.000000"),
            ],
            "a top-level gain is every lane's, snapped to a setting the part can hold"
        );
    }

    #[test]
    fn the_antenna_is_one_switch_set_through_the_first_receiver() {
        let writes = planned(DeviceSettings {
            antenna: Some("B_BALANCED".to_string()),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![channel(false, "voltage0", RF_PORT_SELECT, "B_BALANCED")]
        );
        assert!(
            refused(DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 1,
                    antenna: Some("B_BALANCED".to_string()),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            })
            .contains("share one antenna")
        );
    }

    #[test]
    fn a_lane_of_its_own_is_written_after_the_base_every_lane_shares() {
        let writes = planned(DeviceSettings {
            gains: vec![GainValue {
                stage: RX_STAGE.to_string(),
                value_db: 30.0,
            }],
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 20.0,
                }],
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", HARDWAREGAIN, "30.000000"),
                channel(false, "voltage1", HARDWAREGAIN, "30.000000"),
                channel(false, "voltage1", HARDWAREGAIN, "20.000000"),
            ]
        );
    }

    #[test]
    fn a_per_lane_setting_reaches_that_lane_and_no_other() {
        let writes = planned(DeviceSettings {
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 20.0,
                }],
                agc: Some(AgcSetting::in_mode(true, "fast_attack")),
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage1", HARDWAREGAIN, "20.000000"),
                channel(false, "voltage1", GAIN_CONTROL_MODE, "fast_attack"),
            ]
        );
    }

    #[test]
    fn a_gain_mode_reaches_every_receive_lane_at_once() {
        let writes = planned(DeviceSettings {
            agc: Some(AgcSetting::in_mode(true, "slow_attack")),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![
                channel(false, "voltage0", GAIN_CONTROL_MODE, "slow_attack"),
                channel(false, "voltage1", GAIN_CONTROL_MODE, "slow_attack"),
            ]
        );
    }

    #[test]
    fn automatic_gain_off_is_manual_and_on_without_a_mode_takes_the_first() {
        assert_eq!(
            planned(DeviceSettings {
                agc: Some(AgcSetting::off()),
                ..DeviceSettings::default()
            })[0],
            channel(false, "voltage0", GAIN_CONTROL_MODE, "manual")
        );
        assert_eq!(
            planned(DeviceSettings {
                agc: Some(AgcSetting::switched(true)),
                ..DeviceSettings::default()
            })[0],
            channel(false, "voltage0", GAIN_CONTROL_MODE, "fast_attack")
        );
        assert!(
            refused(DeviceSettings {
                agc: Some(AgcSetting::in_mode(true, "telepathy")),
                ..DeviceSettings::default()
            })
            .contains("no telepathy mode")
        );
    }

    #[test]
    fn a_correction_switch_is_one_for_the_whole_part() {
        let writes = planned(DeviceSettings {
            extra: vec![ExtraValue {
                name: QUADRATURE.to_string(),
                value: json!(false),
            }],
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![channel(false, "voltage0", QUADRATURE_TRACKING, "0")]
        );
    }

    #[test]
    fn parts_per_million_become_the_crystal_correction_the_board_was_trimmed_from() {
        let writes = planned(DeviceSettings {
            ppm: Some(10.0),
            ..DeviceSettings::default()
        });
        assert_eq!(
            writes,
            vec![Write::Device {
                attr: XO_CORRECTION.to_string(),
                value: "40000400".to_string(),
            }]
        );
    }

    #[test]
    fn a_setting_the_radio_cannot_hold_is_refused_by_name() {
        assert!(
            refused(DeviceSettings {
                center_hz: Some(20e9),
                ..DeviceSettings::default()
            })
            .contains("tuning range")
        );
        assert!(
            refused(DeviceSettings {
                sample_rate: Some(200e6),
                ..DeviceSettings::default()
            })
            .contains("converts")
        );
        assert!(
            refused(DeviceSettings {
                bandwidth: Some(BandwidthSetting::Manual { hz: 100e6 }),
                ..DeviceSettings::default()
            })
            .contains("analog filter")
        );
        assert!(
            refused(DeviceSettings {
                bandwidth: Some(BandwidthSetting::Auto),
                ..DeviceSettings::default()
            })
            .contains("own filter")
        );
        assert!(
            refused(DeviceSettings {
                antenna: Some("SMA".to_string()),
                ..DeviceSettings::default()
            })
            .contains("A_BALANCED")
        );
        assert!(
            refused(DeviceSettings {
                ppm: Some(5_000.0),
                ..DeviceSettings::default()
            })
            .contains("crystal can be pulled")
        );
        assert!(
            refused(DeviceSettings {
                gains: vec![GainValue {
                    stage: "LNA".to_string(),
                    value_db: 1.0,
                }],
                ..DeviceSettings::default()
            })
            .contains("no LNA gain stage")
        );
    }

    #[test]
    fn an_extra_of_the_wrong_shape_is_refused_rather_than_coerced() {
        assert!(
            refused(DeviceSettings {
                extra: vec![ExtraValue {
                    name: TX_PORT.to_string(),
                    value: json!("C"),
                }],
                ..DeviceSettings::default()
            })
            .contains("A, B")
        );
        assert!(
            refused(DeviceSettings {
                extra: vec![ExtraValue {
                    name: QUADRATURE.to_string(),
                    value: json!("yes"),
                }],
                ..DeviceSettings::default()
            })
            .contains("on or off")
        );
        assert!(
            refused(DeviceSettings {
                extra: vec![ExtraValue {
                    name: "loopback".to_string(),
                    value: json!(true),
                }],
                ..DeviceSettings::default()
            })
            .contains("no loopback setting")
        );
    }

    #[test]
    fn the_settings_that_come_back_carry_what_the_hardware_was_snapped_to() {
        let layout = crate::layout::tests::two_by_two_layout();
        let front = front();
        let capabilities = capabilities(&front, &layout);
        let (next, _) = plan(
            &DeviceSettings {
                center_hz: Some(100e6),
                gains: vec![GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 40.4,
                }],
                ..DeviceSettings::default()
            },
            &capabilities,
            &front,
            &layout,
            &DeviceSettings {
                sample_rate: Some(2.4e6),
                ..DeviceSettings::default()
            },
        )
        .expect("planned");
        assert_eq!(next.center_hz, Some(100e6));
        assert_eq!(
            next.sample_rate,
            Some(2.4e6),
            "what was set before survives"
        );
        assert_eq!(next.gains[0].value_db, 40.0);
    }

    #[test]
    fn a_value_for_every_lane_clears_what_a_lane_held_apart_unless_it_is_set_apart_again() {
        let layout = crate::layout::tests::two_by_two_layout();
        let front = front();
        let capabilities = capabilities(&front, &layout);
        let held = DeviceSettings {
            agc: Some(AgcSetting::off()),
            gains: vec![GainValue {
                stage: RX_STAGE.to_string(),
                value_db: 40.0,
            }],
            streams: vec![sdrmm_wire::StreamSettings {
                stream: 1,
                gains: vec![GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 20.0,
                }],
                agc: Some(AgcSetting::in_mode(true, "slow_attack")),
                ..sdrmm_wire::StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        let (next, _) = plan(
            &DeviceSettings {
                agc: Some(AgcSetting::off()),
                gains: vec![GainValue {
                    stage: RX_STAGE.to_string(),
                    value_db: 30.0,
                }],
                streams: vec![sdrmm_wire::StreamSettings {
                    stream: 1,
                    gains: vec![GainValue {
                        stage: RX_STAGE.to_string(),
                        value_db: 10.0,
                    }],
                    ..sdrmm_wire::StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
            &capabilities,
            &front,
            &layout,
            &held,
        )
        .expect("planned");
        let lane = next.for_stream(1, &capabilities.per_stream);
        assert_eq!(lane.agc, Some(AgcSetting::off()));
        assert_eq!(lane.gains[0].value_db, 10.0);
    }

    #[test]
    fn a_lane_the_radio_does_not_have_is_refused() {
        let layout = crate::layout::tests::one_by_one_layout();
        let front = front();
        let capabilities = capabilities(&front, &layout);
        let error = plan(
            &DeviceSettings {
                streams: vec![sdrmm_wire::StreamSettings {
                    stream: 1,
                    ..sdrmm_wire::StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
            &capabilities,
            &front,
            &layout,
            &DeviceSettings::default(),
        )
        .expect_err("refused");
        assert!(error.to_string().contains("streams[1]"), "{error}");
    }
}

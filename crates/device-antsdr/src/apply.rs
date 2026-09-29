use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    AgcSetting, BandwidthSetting, Capabilities, DeviceSettings, ExtraValue, GainKind, GainValue,
    any_range_holds,
};

use crate::{
    ad9361::Tracking,
    board::Board,
    caps::{self, BB_DC, QUADRATURE, TX_GAIN},
};

const MAX_PPM: f64 = 100.0;

pub(crate) fn initial(
    board: &Board,
    capabilities: &Capabilities,
    rate: f64,
    gain_db: f64,
) -> DeviceSettings {
    DeviceSettings {
        center_hz: Some(board.frequency().round()),
        sample_rate: Some(rate),
        ppm: Some(0.0),
        bandwidth: Some(BandwidthSetting::Auto),
        agc: Some(AgcSetting::off()),
        gains: vec![
            GainValue::new(GainKind::Tuner, gain_db),
            GainValue::new(GainKind::Tx, TX_GAIN.min),
        ],
        extra: vec![flag(QUADRATURE, true), flag(BB_DC, true)],
        rx_streams: (!capabilities.rx_stream_choices.is_empty()).then_some(capabilities.rx_streams),
        ..DeviceSettings::default()
    }
}

fn flag(name: &str, on: bool) -> ExtraValue {
    ExtraValue {
        name: name.to_string(),
        value: serde_json::Value::Bool(on),
    }
}

pub(crate) fn apply(
    board: &mut Board,
    capabilities: &mut Capabilities,
    settings: &mut DeviceSettings,
    delta: &DeviceSettings,
    streaming: bool,
) -> Result<(), DeviceError> {
    refuse_unoffered(delta)?;
    if let Some(lanes) = delta.rx_streams {
        set_lanes(board, capabilities, settings, lanes, streaming)?;
    }
    check_stream_settings(delta, capabilities)?;
    if let Some(rate) = delta.sample_rate {
        set_rate(
            board,
            capabilities,
            settings,
            rate,
            delta.bandwidth.is_none(),
        )?;
    }
    if let Some(bandwidth) = delta.bandwidth {
        set_bandwidth(board, capabilities, settings, bandwidth)?;
    }
    if let Some(hz) = delta.center_hz {
        if !any_range_holds(&capabilities.freq_ranges, hz) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} is outside this radio's tuning range"
            )));
        }
        board.tune(hz)?;
        settings.center_hz = Some(hz);
    }
    if let Some(ppm) = delta.ppm {
        if !ppm.is_finite() || ppm.abs() > MAX_PPM {
            return Err(DeviceError::Unsupported(format!(
                "ppm {ppm} is outside ±{MAX_PPM}"
            )));
        }
        board.set_ppm(ppm)?;
        settings.ppm = Some(ppm);
    }
    set_lane_controls(board, capabilities, settings, delta)?;
    set_extras(board, settings, delta)?;
    let rest = DeviceSettings {
        tuning: delta.tuning,
        offset_hz: delta.offset_hz,
        dc_block: delta.dc_block,
        ..DeviceSettings::default()
    };
    settings.merge_from(&rest);
    Ok(())
}

fn refuse_unoffered(delta: &DeviceSettings) -> Result<(), DeviceError> {
    if delta.antenna.is_some() || delta.streams.iter().any(|s| s.antenna.is_some()) {
        return Err(DeviceError::Unsupported(
            "this radio's ports are fixed".to_string(),
        ));
    }
    if delta.bias_tee == Some(true) {
        return Err(DeviceError::Unsupported(
            "this radio has no bias tee".to_string(),
        ));
    }
    Ok(())
}

fn set_lanes(
    board: &mut Board,
    capabilities: &mut Capabilities,
    settings: &mut DeviceSettings,
    lanes: u32,
    streaming: bool,
) -> Result<(), DeviceError> {
    if lanes == capabilities.rx_streams {
        return Ok(());
    }
    if !capabilities.rx_stream_choices.contains(&lanes) {
        return Err(DeviceError::Unsupported(format!(
            "this radio streams {:?} lanes, got {lanes}",
            capabilities.rx_stream_choices
        )));
    }
    if streaming {
        tracing::debug!(lanes, "lane count changes when the stream restarts");
    }
    let rate = settings.sample_rate.unwrap_or(board.rate());
    board.set_rate(rate, lanes as usize)?;
    caps::set_lanes(capabilities, lanes as usize);
    settings.rx_streams = Some(lanes);
    settings.streams.retain(|stream| stream.stream < lanes);
    Ok(())
}

fn set_rate(
    board: &mut Board,
    capabilities: &Capabilities,
    settings: &mut DeviceSettings,
    rate: f64,
    follow_bandwidth: bool,
) -> Result<(), DeviceError> {
    if !any_range_holds(&capabilities.sample_rate_ranges, rate) {
        return Err(DeviceError::Unsupported(format!(
            "sample_rate {rate} Hz is outside what this radio streams on {} lanes",
            capabilities.rx_streams
        )));
    }
    board.set_rate(rate, capabilities.rx_streams as usize)?;
    settings.sample_rate = Some(rate);
    let automatic = settings.bandwidth.is_none_or(BandwidthSetting::is_auto);
    if follow_bandwidth && automatic {
        board.set_bandwidth(rate)?;
    }
    Ok(())
}

fn set_bandwidth(
    board: &mut Board,
    capabilities: &Capabilities,
    settings: &mut DeviceSettings,
    bandwidth: BandwidthSetting,
) -> Result<(), DeviceError> {
    let hz = match bandwidth {
        BandwidthSetting::Auto => settings
            .sample_rate
            .unwrap_or(capabilities.bandwidth_ranges[0].min),
        BandwidthSetting::Manual { hz } => {
            if !any_range_holds(&capabilities.bandwidth_ranges, hz) {
                return Err(DeviceError::Unsupported(format!(
                    "bandwidth {hz} Hz is outside this radio's analog filter"
                )));
            }
            hz
        }
    };
    board.set_bandwidth(hz)?;
    settings.bandwidth = Some(bandwidth);
    Ok(())
}

fn set_lane_controls(
    board: &mut Board,
    capabilities: &Capabilities,
    settings: &mut DeviceSettings,
    delta: &DeviceSettings,
) -> Result<(), DeviceError> {
    let radios = board.radios();
    if let Some(agc) = &delta.agc {
        check_agc(capabilities, agc)?;
        for lane in 0..radios {
            board.set_gain_mode(lane, caps::gain_mode(agc.on, agc.mode.as_deref()))?;
        }
    }
    let mut applied = Vec::new();
    for gain in &delta.gains {
        let mut value = gain.value_db;
        for lane in 0..radios {
            value = set_gain(board, lane, gain)?;
        }
        applied.push(GainValue {
            stage: gain.stage.clone(),
            value_db: value,
        });
    }
    let mut lanes = delta.streams.clone();
    for stream in &mut lanes {
        let lane = stream.stream as usize;
        if let Some(agc) = &stream.agc {
            check_agc(capabilities, agc)?;
            board.set_gain_mode(lane, caps::gain_mode(agc.on, agc.mode.as_deref()))?;
        }
        for gain in &mut stream.gains {
            gain.value_db = set_gain(board, lane, gain)?;
        }
    }
    let merged = DeviceSettings {
        agc: delta.agc.clone(),
        gains: applied,
        streams: lanes,
        ..DeviceSettings::default()
    };
    settings.merge_from(&merged);
    settle_lanes(settings, delta);
    Ok(())
}

fn check_agc(capabilities: &Capabilities, agc: &AgcSetting) -> Result<(), DeviceError> {
    if capabilities.agc.admits(agc) {
        Ok(())
    } else {
        Err(DeviceError::Unsupported(format!(
            "agc mode {:?} is not one this radio offers",
            agc.mode
        )))
    }
}

fn set_gain(board: &mut Board, lane: usize, gain: &GainValue) -> Result<f64, DeviceError> {
    if !gain.value_db.is_finite() {
        return Err(DeviceError::Unsupported(format!(
            "{} gain {} is not a number",
            gain.stage, gain.value_db
        )));
    }
    if gain.stage == GainKind::Tuner.name() {
        board.set_rx_gain(lane, gain.value_db)
    } else if gain.stage == GainKind::Tx.name() {
        board.set_tx_gain(lane, gain.value_db)
    } else {
        Err(DeviceError::Unsupported(format!(
            "this radio has no {} gain",
            gain.stage
        )))
    }
}

fn settle_lanes(settings: &mut DeviceSettings, delta: &DeviceSettings) {
    for stream in &mut settings.streams {
        let own = delta.streams.iter().find(|s| s.stream == stream.stream);
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

fn set_extras(
    board: &mut Board,
    settings: &mut DeviceSettings,
    delta: &DeviceSettings,
) -> Result<(), DeviceError> {
    if delta.extra.is_empty() {
        return Ok(());
    }
    for extra in &delta.extra {
        if extra.name != QUADRATURE && extra.name != BB_DC {
            return Err(DeviceError::Unsupported(format!(
                "this radio has no setting {}",
                extra.name
            )));
        }
        if !extra.value.is_boolean() {
            return Err(DeviceError::Unsupported(format!(
                "{} takes true or false",
                extra.name
            )));
        }
    }
    let mut next = settings.clone();
    next.merge_from(&DeviceSettings {
        extra: delta.extra.clone(),
        ..DeviceSettings::default()
    });
    let on = |name: &str| {
        next.extra
            .iter()
            .find(|extra| extra.name == name)
            .and_then(|extra| extra.value.as_bool())
            .unwrap_or(true)
    };
    board.set_tracking(Tracking {
        quadrature: on(QUADRATURE),
        dc: on(BB_DC),
    })?;
    settings.extra = next.extra;
    Ok(())
}

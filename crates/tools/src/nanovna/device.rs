use std::time::Instant;

use sdrmm_wire::{
    NanoVnaCalStep, NanoVnaCalibration, NanoVnaComplex, NanoVnaDeviceReport, NanoVnaPoint,
    NanoVnaStandard, NanoVnaSweep, NanoVnaSweepRequest, NanoVnaSweepState,
};

use super::shell::{Session, first_number, parse_complex, parse_frequencies, reported_value};

/// The firmware's own per-scan ceiling. Wider sweeps are stitched from segments of at most this
/// many points, which every build in the family accepts.
const SEGMENT_POINTS: u32 = 101;

const APPLIED_TOKEN: &str = "cal'ed";

/// Everything the instrument will say about itself. A field a given firmware has no command for
/// is left empty; a port that cannot be talked to at all fails the whole read.
pub fn describe(session: &mut Session<'_>, port: &str) -> Result<NanoVnaDeviceReport, String> {
    let firmware = session.command("version")?.join(" ");
    if firmware.is_empty() {
        return Err("the device returned no firmware version".to_owned());
    }
    let info = session.command("info")?;
    let board = info
        .iter()
        .find_map(|line| line.strip_prefix("Board:"))
        .map(|board| board.trim().to_owned());
    let battery_mv = value(session, "vbat")?
        .as_deref()
        .and_then(first_number)
        .and_then(|value| u32::try_from(value).ok());
    let bandwidth_hz = bandwidth(session)?;
    let power = value(session, "power")?
        .as_deref()
        .and_then(first_number)
        .and_then(|value| u16::try_from(value).ok());
    let tcxo_hz = value(session, "tcxo")?.as_deref().and_then(first_number);
    let harmonic_threshold_hz = value(session, "threshold")?
        .as_deref()
        .and_then(first_number);
    let electrical_delay_s = value(session, "edelay")?.and_then(|text| text.parse().ok());
    let s21_offset_db = value(session, "s21offset")?.and_then(|text| text.parse().ok());
    let sweep = sweep_state(session)?;
    let calibration = calibration_status(session, port)?;
    let commands = session
        .command("help")?
        .iter()
        .find_map(|line| line.strip_prefix("Commands:"))
        .map(|list| list.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    Ok(NanoVnaDeviceReport {
        port: port.to_owned(),
        firmware,
        board,
        info,
        battery_mv,
        bandwidth_hz,
        power,
        tcxo_hz,
        harmonic_threshold_hz,
        electrical_delay_s,
        s21_offset_db,
        sweep,
        calibration,
        commands,
    })
}

fn value(session: &mut Session<'_>, request: &str) -> Result<Option<String>, String> {
    let lines = session.command(request)?;
    Ok(reported_value(&lines).map(str::to_owned))
}

/// `bandwidth` answers with an index and the resolution it selects — `bandwidth 3 (1000Hz)` —
/// and it is the resolution, not the index, that describes the measurement.
fn bandwidth(session: &mut Session<'_>) -> Result<Option<u32>, String> {
    let lines = session.command("bandwidth")?;
    let Some(line) = lines.iter().find(|line| line.contains('(')) else {
        return Ok(None);
    };
    let inside = line
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(inside, _)| inside);
    Ok(inside
        .and_then(first_number)
        .and_then(|value| u32::try_from(value).ok()))
}

fn sweep_state(session: &mut Session<'_>) -> Result<Option<NanoVnaSweepState>, String> {
    let lines = session.command("sweep")?;
    let Some(fields) = lines
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| fields.len() == 3)
    else {
        return Ok(None);
    };
    let (Ok(start_hz), Ok(stop_hz), Ok(points)) = (
        fields[0].parse::<u64>(),
        fields[1].parse::<u64>(),
        fields[2].parse::<u32>(),
    ) else {
        return Ok(None);
    };
    Ok(Some(NanoVnaSweepState {
        start_hz,
        stop_hz,
        points,
    }))
}

pub fn calibration_status(
    session: &mut Session<'_>,
    port: &str,
) -> Result<NanoVnaCalibration, String> {
    let raw = session.command("cal")?.join(" ");
    let mut standards = Vec::new();
    let mut error_terms = Vec::new();
    let mut applied = false;
    for token in raw.split_whitespace() {
        if token == APPLIED_TOKEN {
            applied = true;
        } else if let Some(standard) = NanoVnaStandard::from_token(token) {
            standards.push(standard);
        } else {
            error_terms.push(token.to_owned());
        }
    }
    Ok(NanoVnaCalibration {
        port: port.to_owned(),
        standards,
        error_terms,
        applied,
        raw: raw.trim().to_owned(),
    })
}

/// Run one calibration move and report the state it left the instrument in, so a panel never has
/// to assume a step took.
pub fn calibrate(
    session: &mut Session<'_>,
    port: &str,
    step: &NanoVnaCalStep,
    range: Option<NanoVnaSweepState>,
) -> Result<NanoVnaCalibration, String> {
    if matches!(step, NanoVnaCalStep::Status) {
        return calibration_status(session, port);
    }
    if let (NanoVnaCalStep::Reset, Some(range)) = (step, range) {
        session.silent(&format!(
            "sweep {} {} {}",
            range.start_hz, range.stop_hz, range.points
        ))?;
    }
    session.silent(&command_for(step))?;
    calibration_status(session, port)
}

fn command_for(step: &NanoVnaCalStep) -> String {
    match step {
        NanoVnaCalStep::Status => "cal".to_owned(),
        NanoVnaCalStep::Reset => "cal reset".to_owned(),
        NanoVnaCalStep::Open => "cal open".to_owned(),
        NanoVnaCalStep::Short => "cal short".to_owned(),
        NanoVnaCalStep::Load => "cal load".to_owned(),
        NanoVnaCalStep::Thru => "cal thru".to_owned(),
        NanoVnaCalStep::Isolation => "cal isoln".to_owned(),
        NanoVnaCalStep::Finish => "cal done".to_owned(),
        NanoVnaCalStep::Enable => "cal on".to_owned(),
        NanoVnaCalStep::Disable => "cal off".to_owned(),
        NanoVnaCalStep::Save { slot } => format!("save {slot}"),
        NanoVnaCalStep::Recall { slot } => format!("recall {slot}"),
    }
}

pub fn acquire(
    session: &mut Session<'_>,
    request: &NanoVnaSweepRequest,
) -> Result<NanoVnaSweep, String> {
    let started = Instant::now();
    let device = describe(session, &request.port)?;
    let mut points = Vec::with_capacity(request.points as usize);
    let mut point_offset = 0;
    for size in segment_sizes(request.points) {
        let segment_start = frequency_at(request, point_offset);
        let segment_stop = frequency_at(request, point_offset + size - 1);
        let values = acquire_segment(session, segment_start, segment_stop, size, request.averages)?;
        points.extend(values);
        point_offset += size;
    }
    session.silent(&format!(
        "sweep {} {} {}",
        request.start_hz,
        request.stop_hz,
        request.points.min(SEGMENT_POINTS)
    ))?;
    session.silent("resume")?;
    if points.len() != request.points as usize {
        return Err(format!(
            "requested {} points but the device returned {}",
            request.points,
            points.len()
        ));
    }
    Ok(NanoVnaSweep {
        device,
        requested_points: request.points,
        averages: request.averages,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        points,
    })
}

fn acquire_segment(
    session: &mut Session<'_>,
    start_hz: u64,
    stop_hz: u64,
    points: u32,
    averages: u16,
) -> Result<Vec<NanoVnaPoint>, String> {
    let mut frequencies = Vec::new();
    let mut s11_sum = vec![NanoVnaComplex { re: 0.0, im: 0.0 }; points as usize];
    let mut s21_sum = vec![NanoVnaComplex { re: 0.0, im: 0.0 }; points as usize];
    for average in 0..averages {
        session.silent(&format!("scan {start_hz} {stop_hz} {points}"))?;
        let next_frequencies = parse_frequencies(session.command("frequencies")?)?;
        let s11 = parse_complex(session.command("data 0")?)?;
        let s21 = parse_complex(session.command("data 1")?)?;
        ensure_lengths(points, &next_frequencies, &s11, &s21)?;
        if average == 0 {
            frequencies = next_frequencies;
        } else if frequencies != next_frequencies {
            return Err("device frequencies changed while averaging".to_owned());
        }
        accumulate(&mut s11_sum, &s11);
        accumulate(&mut s21_sum, &s21);
    }
    let divisor = f64::from(averages);
    Ok(frequencies
        .into_iter()
        .zip(s11_sum)
        .zip(s21_sum)
        .map(|((frequency_hz, s11), s21)| NanoVnaPoint {
            frequency_hz,
            s11: divide(s11, divisor),
            s21: divide(s21, divisor),
        })
        .collect())
}

fn segment_sizes(points: u32) -> Vec<u32> {
    let segment_count = points.div_ceil(SEGMENT_POINTS);
    let base_size = points / segment_count;
    let extra = points % segment_count;
    (0..segment_count)
        .map(|segment| base_size + u32::from(segment < extra))
        .collect()
}

fn ensure_lengths(
    expected: u32,
    frequencies: &[u64],
    s11: &[NanoVnaComplex],
    s21: &[NanoVnaComplex],
) -> Result<(), String> {
    let expected = expected as usize;
    if frequencies.len() == expected && s11.len() == expected && s21.len() == expected {
        return Ok(());
    }
    Err(format!(
        "device returned {} frequencies, {} S11 values, and {} S21 values; expected {expected}",
        frequencies.len(),
        s11.len(),
        s21.len()
    ))
}

fn accumulate(sum: &mut [NanoVnaComplex], values: &[NanoVnaComplex]) {
    for (sum, value) in sum.iter_mut().zip(values) {
        sum.re += value.re;
        sum.im += value.im;
    }
}

fn divide(value: NanoVnaComplex, divisor: f64) -> NanoVnaComplex {
    NanoVnaComplex {
        re: value.re / divisor,
        im: value.im / divisor,
    }
}

fn frequency_at(request: &NanoVnaSweepRequest, index: u32) -> u64 {
    let span = u128::from(request.stop_hz - request.start_hz);
    let numerator = span * u128::from(index);
    let denominator = u128::from(request.points - 1);
    request.start_hz + ((numerator + denominator / 2) / denominator) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_distribution_keeps_every_chunk_within_device_limits() {
        let sizes = segment_sizes(102);
        assert_eq!(sizes, vec![51, 51]);
        assert!(sizes.into_iter().all(|size| size <= SEGMENT_POINTS));
        assert_eq!(segment_sizes(11), vec![11]);
        assert!(
            segment_sizes(10_001)
                .into_iter()
                .all(|size| size <= SEGMENT_POINTS)
        );
    }

    #[test]
    fn each_calibration_step_names_its_own_shell_command() {
        assert_eq!(command_for(&NanoVnaCalStep::Isolation), "cal isoln");
        assert_eq!(command_for(&NanoVnaCalStep::Finish), "cal done");
        assert_eq!(command_for(&NanoVnaCalStep::Save { slot: 3 }), "save 3");
    }
}

use std::f64::consts::PI;

use sdrmm_wire::tools::{
    NanoVnaCalibration, NanoVnaComplex, NanoVnaDeviceReport, NanoVnaPoint, NanoVnaStandard,
    NanoVnaSweep, NanoVnaSweepState,
};

use super::rf::complex;

pub fn device() -> NanoVnaDeviceReport {
    NanoVnaDeviceReport {
        port: "/dev/cu.usbmodem4001".to_owned(),
        firmware: "1.2.46".to_owned(),
        board: Some("NanoVNA-H 4".to_owned()),
        info: vec![
            "Board: NanoVNA-H 4".to_owned(),
            "Platform: STM32F303xC Analog & DSP".to_owned(),
        ],
        battery_mv: Some(4177),
        bandwidth_hz: Some(1000),
        power: Some(255),
        tcxo_hz: Some(26_000_000),
        harmonic_threshold_hz: Some(300_000_100),
        electrical_delay_s: Some(0.0),
        s21_offset_db: Some(0.0),
        sweep: Some(NanoVnaSweepState {
            start_hz: 50_000,
            stop_hz: 900_000_000,
            points: 101,
        }),
        calibration: NanoVnaCalibration {
            port: "/dev/cu.usbmodem4001".to_owned(),
            standards: vec![NanoVnaStandard::Load, NanoVnaStandard::Isolation],
            error_terms: vec!["Es".to_owned(), "Er".to_owned(), "Et".to_owned()],
            applied: true,
            raw: "load isoln Es Er Et cal'ed".to_owned(),
        },
        commands: ["scan", "data", "frequencies", "sweep", "cal"]
            .map(str::to_owned)
            .to_vec(),
    }
}

pub fn point(frequency_hz: u64, s11: NanoVnaComplex, s21: NanoVnaComplex) -> NanoVnaPoint {
    NanoVnaPoint {
        frequency_hz,
        s11,
        s21,
    }
}

pub fn sweep_of(points: Vec<NanoVnaPoint>) -> NanoVnaSweep {
    NanoVnaSweep {
        device: device(),
        requested_points: points.len() as u32,
        averages: 1,
        elapsed_ms: 1234,
        points,
    }
}

pub fn resonant_sweep(count: usize) -> NanoVnaSweep {
    let centre_hz = 14_100_000.0;
    let resistance = 50.0;
    let inductance = 1e-5;
    let capacitance = 1.0 / (inductance * (2.0 * PI * centre_hz).powi(2));
    let points = (0..count)
        .map(|index| {
            let frequency_hz = 13_000_000.0 + index as f64 / (count - 1) as f64 * 2_000_000.0;
            let omega = 2.0 * PI * frequency_hz;
            let reactance = omega * inductance - 1.0 / (omega * capacitance);
            point(
                frequency_hz.round() as u64,
                reflection(resistance, reactance),
                complex(0.0, 0.0),
            )
        })
        .collect();
    sweep_of(points)
}

fn reflection(resistance: f64, reactance: f64) -> NanoVnaComplex {
    let reference = 50.0;
    let numerator_re = resistance - reference;
    let denominator_re = resistance + reference;
    let denominator = denominator_re * denominator_re + reactance * reactance;
    complex(
        (numerator_re * denominator_re + reactance * reactance) / denominator,
        (reactance * denominator_re - numerator_re * reactance) / denominator,
    )
}

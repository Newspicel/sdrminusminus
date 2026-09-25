use std::f64::consts::PI;

use sdrmm_wire::tools::{
    NanoVnaCalStep, NanoVnaCalibrateRequest, NanoVnaCalibration, NanoVnaComplex, NanoVnaDevice,
    NanoVnaDeviceReport, NanoVnaPoint, NanoVnaPortRequest, NanoVnaRequest, NanoVnaResult,
    NanoVnaSweep, NanoVnaSweepRequest, NanoVnaSweepState, ToolRequest, ToolResponse,
};

pub const REFERENCE_OHMS: f64 = 50.0;

#[must_use]
pub fn devices_request() -> ToolRequest {
    ToolRequest::NanoVna(NanoVnaRequest::ListDevices)
}

#[must_use]
pub fn describe_request(port: &str) -> ToolRequest {
    ToolRequest::NanoVna(NanoVnaRequest::Describe(NanoVnaPortRequest {
        port: port.to_owned(),
    }))
}

#[must_use]
pub fn sweep_request(request: NanoVnaSweepRequest) -> ToolRequest {
    ToolRequest::NanoVna(NanoVnaRequest::Sweep(request))
}

#[must_use]
pub fn calibrate_request(
    port: &str,
    step: NanoVnaCalStep,
    range: Option<NanoVnaSweepState>,
) -> ToolRequest {
    ToolRequest::NanoVna(NanoVnaRequest::Calibrate(NanoVnaCalibrateRequest {
        port: port.to_owned(),
        range,
        step,
    }))
}

fn result_of(response: Option<&ToolResponse>) -> Option<&NanoVnaResult> {
    match response? {
        ToolResponse::NanoVna(result) => Some(result),
        _ => None,
    }
}

#[must_use]
pub fn devices_of(response: Option<&ToolResponse>) -> &[NanoVnaDevice] {
    match result_of(response) {
        Some(NanoVnaResult::Devices { devices, .. }) => devices,
        _ => &[],
    }
}

#[must_use]
pub fn ignored_ports_of(response: Option<&ToolResponse>) -> &[String] {
    match result_of(response) {
        Some(NanoVnaResult::Devices { ignored_ports, .. }) => ignored_ports,
        _ => &[],
    }
}

#[must_use]
pub fn report_of(response: Option<&ToolResponse>) -> Option<&NanoVnaDeviceReport> {
    match result_of(response) {
        Some(NanoVnaResult::Device(report)) => Some(report),
        _ => None,
    }
}

#[must_use]
pub fn sweep_of(response: Option<&ToolResponse>) -> Option<&NanoVnaSweep> {
    match result_of(response) {
        Some(NanoVnaResult::Sweep(sweep)) => Some(sweep),
        _ => None,
    }
}

#[must_use]
pub fn calibration_of(response: Option<&ToolResponse>) -> Option<&NanoVnaCalibration> {
    match result_of(response) {
        Some(NanoVnaResult::Calibration(calibration)) => Some(calibration),
        _ => None,
    }
}

#[must_use]
pub fn complex(re: f64, im: f64) -> NanoVnaComplex {
    NanoVnaComplex { re, im }
}

#[must_use]
pub fn magnitude(value: NanoVnaComplex) -> f64 {
    value.re.hypot(value.im)
}

#[must_use]
pub fn gain_db(value: NanoVnaComplex) -> f64 {
    let absolute = magnitude(value);
    if absolute > 0.0 {
        20.0 * absolute.log10()
    } else {
        f64::NEG_INFINITY
    }
}

#[must_use]
pub fn return_loss_db(value: NanoVnaComplex) -> f64 {
    -gain_db(value)
}

#[must_use]
pub fn phase_deg(value: NanoVnaComplex) -> f64 {
    value.im.atan2(value.re).to_degrees()
}

#[must_use]
pub fn vswr(value: NanoVnaComplex) -> f64 {
    let gamma = magnitude(value);
    if gamma < 1.0 {
        (1.0 + gamma) / (1.0 - gamma)
    } else {
        f64::INFINITY
    }
}

#[must_use]
pub fn mismatch_loss_db(value: NanoVnaComplex) -> f64 {
    let gamma = magnitude(value);
    let transmitted = 1.0 - gamma * gamma;
    if transmitted > 0.0 {
        -10.0 * transmitted.log10()
    } else {
        f64::INFINITY
    }
}

#[must_use]
pub fn impedance(value: NanoVnaComplex) -> Option<NanoVnaComplex> {
    let denominator_re = 1.0 - value.re;
    let denominator_im = -value.im;
    let denominator = denominator_re * denominator_re + denominator_im * denominator_im;
    if denominator == 0.0 {
        return None;
    }
    let numerator_re = 1.0 + value.re;
    let numerator_im = value.im;
    Some(complex(
        REFERENCE_OHMS * (numerator_re * denominator_re + numerator_im * denominator_im)
            / denominator,
        REFERENCE_OHMS * (numerator_im * denominator_re - numerator_re * denominator_im)
            / denominator,
    ))
}

#[must_use]
pub fn admittance(value: NanoVnaComplex) -> Option<NanoVnaComplex> {
    let z = impedance(value)?;
    let squared = z.re * z.re + z.im * z.im;
    if squared == 0.0 {
        return None;
    }
    Some(complex(z.re / squared, -z.im / squared))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComponentKind {
    Capacitance,
    Inductance,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Component {
    pub kind: ComponentKind,
    pub value: f64,
}

#[must_use]
pub fn equivalent_component(reactance_ohms: f64, frequency_hz: f64) -> Option<Component> {
    if !reactance_ohms.is_finite() || frequency_hz <= 0.0 || reactance_ohms == 0.0 {
        return None;
    }
    let omega = 2.0 * PI * frequency_hz;
    Some(if reactance_ohms < 0.0 {
        Component {
            kind: ComponentKind::Capacitance,
            value: -1.0 / (omega * reactance_ohms),
        }
    } else {
        Component {
            kind: ComponentKind::Inductance,
            value: reactance_ohms / omega,
        }
    })
}

#[must_use]
pub fn q_factor(z: Option<NanoVnaComplex>) -> f64 {
    match z {
        Some(z) if z.re != 0.0 => (z.im / z.re).abs(),
        _ => f64::INFINITY,
    }
}

#[must_use]
pub fn unwrapped_phase(
    points: &[NanoVnaPoint],
    pick: impl Fn(&NanoVnaPoint) -> NanoVnaComplex,
) -> Vec<f64> {
    let mut previous = 0.0;
    let mut offset = 0.0;
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let value = pick(point);
            let raw = value.im.atan2(value.re);
            if index > 0 {
                let delta = raw + offset - previous;
                if delta > PI {
                    offset -= 2.0 * PI;
                } else if delta < -PI {
                    offset += 2.0 * PI;
                }
            }
            previous = raw + offset;
            previous
        })
        .collect()
}

#[must_use]
pub fn group_delays(points: &[NanoVnaPoint]) -> Vec<f64> {
    let phases = unwrapped_phase(points, |point| point.s21);
    let last = points.len().saturating_sub(1);
    (0..points.len())
        .map(|index| {
            let low = index.saturating_sub(1);
            let high = (index + 1).min(last);
            if high == low {
                return f64::NAN;
            }
            let delta_omega =
                2.0 * PI * (points[high].frequency_hz as f64 - points[low].frequency_hz as f64);
            if delta_omega == 0.0 {
                f64::NAN
            } else {
                -(phases[high] - phases[low]) / delta_omega
            }
        })
        .collect()
}

#[must_use]
pub fn lowest_vswr_index(points: &[NanoVnaPoint]) -> usize {
    let mut best = 0;
    for (index, point) in points.iter().enumerate().skip(1) {
        if vswr(point.s11) < vswr(points[best].s11) {
            best = index;
        }
    }
    best
}

#[must_use]
pub fn format_db(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2} dB")
    } else if value > 0.0 {
        "\u{221e} dB".to_owned()
    } else {
        "\u{2212}\u{221e} dB".to_owned()
    }
}

#[must_use]
pub fn format_vswr(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.3}:1")
    } else {
        "\u{221e}".to_owned()
    }
}

#[must_use]
pub fn format_impedance(value: Option<NanoVnaComplex>) -> String {
    match value {
        Some(z) if z.re.is_finite() && z.im.is_finite() => {
            let sign = if z.im < 0.0 { "\u{2212}" } else { "+" };
            format!("{:.1} {sign} j{:.1} \u{3a9}", z.re, z.im.abs())
        }
        _ => "-".to_owned(),
    }
}

const SI_DOWN: [(f64, &str); 6] = [
    (1.0, ""),
    (1e-3, "m"),
    (1e-6, "\u{b5}"),
    (1e-9, "n"),
    (1e-12, "p"),
    (1e-15, "f"),
];

#[must_use]
pub fn format_si(value: f64, unit: &str, digits: usize) -> String {
    if !value.is_finite() {
        return "-".to_owned();
    }
    if value == 0.0 {
        return format!("0 {unit}");
    }
    let magnitude = value.abs();
    let (factor, prefix) = SI_DOWN
        .iter()
        .find(|(factor, _)| magnitude >= *factor)
        .copied()
        .unwrap_or((1e-15, "f"));
    format!("{:.digits$} {prefix}{unit}", value / factor)
}

#[must_use]
pub fn format_number(value: f64, digits: usize) -> String {
    if value.is_finite() {
        format!("{value:.digits$}")
    } else {
        "-".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tools::nanovna::testdata::{point, sweep_of as sample};

    fn close(left: f64, right: f64, digits: i32) -> bool {
        (left - right).abs() < 10f64.powi(-digits) / 2.0
    }

    #[test]
    fn every_call_is_tagged_with_the_tool_id() {
        assert_eq!(
            serde_json::to_value(devices_request()).unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": { "action": "list_devices" } })
        );
        assert_eq!(
            serde_json::to_value(describe_request("COM3")).unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": { "action": "describe", "port": "COM3" } })
        );
        let sweep = sweep_request(NanoVnaSweepRequest {
            port: "COM3".to_owned(),
            start_hz: 1_000_000,
            stop_hz: 30_000_000,
            points: 101,
            averages: 2,
        });
        assert_eq!(
            serde_json::to_value(sweep).unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": {
                "action": "sweep", "port": "COM3", "start_hz": 1_000_000, "stop_hz": 30_000_000, "points": 101, "averages": 2
            } })
        );
    }

    #[test]
    fn a_calibration_step_sits_beside_its_port_with_a_range_only_when_given() {
        assert_eq!(
            serde_json::to_value(calibrate_request("COM3", NanoVnaCalStep::Open, None)).unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": { "action": "calibrate", "port": "COM3", "step": "open" } })
        );
        assert_eq!(
            serde_json::to_value(calibrate_request(
                "COM3",
                NanoVnaCalStep::Save { slot: 4 },
                None
            ))
            .unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": { "action": "calibrate", "port": "COM3", "step": "save", "slot": 4 } })
        );
        let range = NanoVnaSweepState {
            start_hz: 1_000_000,
            stop_hz: 30_000_000,
            points: 101,
        };
        assert_eq!(
            serde_json::to_value(calibrate_request(
                "COM3",
                NanoVnaCalStep::Reset,
                Some(range)
            ))
            .unwrap(),
            serde_json::json!({ "tool": "nanovna", "request": {
                "action": "calibrate", "port": "COM3", "step": "reset",
                "range": { "start_hz": 1_000_000, "stop_hz": 30_000_000, "points": 101 }
            } })
        );
    }

    #[test]
    fn only_the_matching_result_is_unwrapped() {
        let devices = ToolResponse::NanoVna(Box::new(NanoVnaResult::Devices {
            devices: Vec::new(),
            ignored_ports: vec!["/dev/cu.gnss".to_owned()],
        }));
        assert!(devices_of(Some(&devices)).is_empty());
        assert_eq!(ignored_ports_of(Some(&devices)), ["/dev/cu.gnss"]);
        assert!(sweep_of(Some(&devices)).is_none());
        assert!(report_of(None).is_none());
    }

    #[test]
    fn a_matched_load_measures_analytically() {
        let matched = complex(0.0, 0.0);
        assert_eq!(vswr(matched), 1.0);
        assert_eq!(impedance(matched), Some(complex(50.0, 0.0)));
        assert_eq!(return_loss_db(matched), f64::INFINITY);
        assert!(close(mismatch_loss_db(matched), 0.0, 12));
    }

    #[test]
    fn magnitude_phase_vswr_and_impedance_follow_from_gamma() {
        let gamma = complex(0.0, 0.5);
        assert!(close(gain_db(gamma), -6.0206, 4));
        assert!(close(return_loss_db(gamma), 6.0206, 4));
        assert_eq!(phase_deg(gamma), 90.0);
        assert!(close(vswr(gamma), 3.0, 12));
        let z = impedance(gamma).unwrap();
        assert!(close(z.re, 30.0, 9) && close(z.im, 40.0, 9));
        assert_eq!(format_impedance(Some(z)), "30.0 + j40.0 \u{3a9}");
        assert!(close(q_factor(Some(z)), 40.0 / 30.0, 6));
    }

    #[test]
    fn a_mismatch_costs_forward_power() {
        assert!(close(
            mismatch_loss_db(complex(0.5, 0.0)),
            -10.0 * 0.75f64.log10(),
            6
        ));
    }

    #[test]
    fn a_reactance_reads_as_the_component_that_makes_it() {
        let capacitive = equivalent_component(-1.0 / (2.0 * PI * 1e6 * 1e-9), 1e6).unwrap();
        assert_eq!(capacitive.kind, ComponentKind::Capacitance);
        assert!(close(capacitive.value, 1e-9, 15));
        let inductive = equivalent_component(2.0 * PI * 1e6 * 1e-6, 1e6).unwrap();
        assert_eq!(inductive.kind, ComponentKind::Inductance);
        assert!(close(inductive.value, 1e-6, 12));
        assert!(equivalent_component(0.0, 1e6).is_none());
    }

    #[test]
    fn the_best_match_in_a_sweep_is_found() {
        let sweep = sample(vec![
            point(1, complex(0.5, 0.0), complex(0.0, 0.0)),
            point(2, complex(0.1, 0.0), complex(0.0, 0.0)),
            point(3, complex(0.3, 0.0), complex(0.0, 0.0)),
        ]);
        assert_eq!(lowest_vswr_index(&sweep.points), 1);
    }

    fn unit(degrees: f64) -> NanoVnaComplex {
        let radians = degrees.to_radians();
        complex(radians.cos(), radians.sin())
    }

    #[test]
    fn phase_unwraps_across_the_seam() {
        let points: Vec<_> = [0.0, 170.0, -170.0, -10.0]
            .iter()
            .enumerate()
            .map(|(index, degrees)| point(index as u64 + 1, complex(0.0, 0.0), unit(*degrees)))
            .collect();
        let degrees: Vec<i64> = unwrapped_phase(&points, |point| point.s21)
            .iter()
            .map(|radians| radians.to_degrees().round() as i64)
            .collect();
        assert_eq!(degrees, [0, 170, 190, 350]);
    }

    #[test]
    fn a_linear_phase_slope_reads_as_a_constant_delay() {
        let delay = 5e-9;
        let points: Vec<_> = (0..5)
            .map(|index| {
                let frequency = 1_000_000 + index * 100_000;
                let radians = -2.0 * PI * frequency as f64 * delay;
                point(
                    frequency,
                    complex(0.0, 0.0),
                    complex(radians.cos(), radians.sin()),
                )
            })
            .collect();
        for value in group_delays(&points) {
            assert!(close(value, delay, 12));
        }
    }

    #[test]
    fn values_scale_to_engineering_units() {
        assert_eq!(format_si(5e-9, "s", 2), "5.00 ns");
        assert_eq!(format_si(1.2e-12, "F", 1), "1.2 pF");
        assert_eq!(format_si(0.0, "s", 3), "0 s");
        assert_eq!(format_si(f64::NAN, "s", 3), "-");
        assert_eq!(format_db(f64::NEG_INFINITY), "\u{2212}\u{221e} dB");
        assert_eq!(format_vswr(f64::INFINITY), "\u{221e}");
    }
}

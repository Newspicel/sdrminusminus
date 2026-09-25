use sdrmm_wire::tools::NanoVnaDeviceReport;
use zgui::prelude::*;

use super::{
    analysis::{Band, PointReadout, SweepAnalysis},
    rf::{ComponentKind, format_db, format_impedance, format_number, format_si, format_vswr},
};
use crate::ui::tools::kit::{format_hz, group, line};

pub fn marker_readout(row: &PointReadout) -> impl IntoView {
    let z = row.impedance;
    let y = row.admittance;
    let ohms = |value: Option<f64>| {
        value.map_or_else(
            || "-".to_owned(),
            |value| format!("{} \u{3a9}", format_number(value, 2)),
        )
    };
    let (component_label, component_value) = match row.component {
        Some(component) if component.kind == ComponentKind::Inductance => {
            ("Series L", format_si(component.value, "H", 3))
        }
        Some(component) => ("Series C", format_si(component.value, "F", 3)),
        None => ("Series C", "-".to_owned()),
    };
    let siemens =
        |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |value| format_si(value, "S", 3));
    let reflection = vec![
        line("VSWR", format_vswr(row.vswr), true),
        line("Return loss", format_db(row.return_loss_db), false),
        line("|S11|", format_db(row.s11_db), false),
        line("|S11| linear", format_number(row.s11_linear, 5), false),
        line(
            "S11 phase",
            format!("{}\u{b0}", format_number(row.s11_phase_deg, 2)),
            false,
        ),
        line("Mismatch loss", format_db(row.mismatch_loss_db), false),
        line("S11 real", format_number(row.s11.re, 6), false),
        line("S11 imag", format_number(row.s11.im, 6), false),
    ];
    let impedance = vec![
        line("Z", format_impedance(z), true),
        line("Resistance", ohms(z.map(|z| z.re)), false),
        line("Reactance", ohms(z.map(|z| z.im)), false),
        line(
            "|Z|",
            format!("{} \u{3a9}", format_number(row.impedance_magnitude, 2)),
            false,
        ),
        line("Q", format_number(row.q, 2), false),
        line(component_label, component_value, false),
        line("Conductance", siemens(y.map(|y| y.re)), false),
        line("Susceptance", siemens(y.map(|y| y.im)), false),
    ];
    let transmission = vec![
        line("S21 gain", format_db(row.s21_db), true),
        line("Insertion loss", format_db(row.insertion_loss_db), false),
        line("|S21| linear", format_number(row.s21_linear, 6), false),
        line(
            "S21 phase",
            format!("{}\u{b0}", format_number(row.s21_phase_deg, 2)),
            false,
        ),
        line("Group delay", format_si(row.group_delay_s, "s", 2), false),
        line("S21 real", format_number(row.s21.re, 6), false),
        line("S21 imag", format_number(row.s21.im, 6), false),
        line("Point", format!("#{}", row.index + 1), false),
    ];
    view! {
        box(class = "tool-groups") {
            {group(format!("Reflection \u{b7} {}", format_hz(row.frequency_hz)), reflection)}
            {group("Impedance", impedance)}
            {group("Transmission", transmission)}
        }
    }
}

#[must_use]
pub fn describe_band(band: &Band) -> String {
    let span = format!(
        "{} \u{2013} {}",
        format_hz(band.start_hz),
        format_hz(band.stop_hz)
    );
    let width = format_hz(band.span_hz);
    if band.truncated {
        format!("{span} (\u{2265} {width}, clipped)")
    } else {
        format!("{span} ({width})")
    }
}

pub fn sweep_summary(analysis: &SweepAnalysis) -> impl IntoView {
    let best = analysis.resonance.as_ref().map_or_else(
        || vec![line("Resonance", "-", false)],
        |resonance| {
            vec![
                line("Frequency", format_hz(resonance.frequency_hz), true),
                line("VSWR", format_vswr(resonance.vswr), false),
                line("Return loss", format_db(resonance.return_loss_db), false),
                line("Z", format_impedance(resonance.impedance), false),
            ]
        },
    );
    let bands = analysis
        .vswr_bands
        .iter()
        .map(|(limit, band)| {
            line(
                format!("VSWR \u{2264} {limit}"),
                band.as_ref()
                    .map_or_else(|| "not reached".to_owned(), describe_band),
                false,
            )
        })
        .collect();
    let through = analysis.peak.as_ref().map_or_else(
        || vec![line("S21", "nothing through CH1", false)],
        |peak| {
            let band = analysis.transmission_band.as_ref();
            vec![
                line("Peak", format_db(peak.s21_db), true),
                line("At", format_hz(peak.frequency_hz), false),
                line(
                    "\u{2212}3 dB band",
                    band.map_or_else(|| "not reached".to_owned(), describe_band),
                    false,
                ),
                line(
                    "Loaded Q",
                    band.map_or_else(|| "-".to_owned(), |band| format_number(band.q, 1)),
                    false,
                ),
            ]
        },
    );
    view! {
        box(class = "tool-groups") {
            {group("Best match", best)}
            {group("Usable bandwidth", bands)}
            {group("Transmission", through)}
        }
    }
}

#[must_use]
pub fn describe_power(power: Option<u16>) -> String {
    match power {
        None => "-".to_owned(),
        Some(255) => "auto".to_owned(),
        Some(level) => level.to_string(),
    }
}

pub fn device_report(report: &NanoVnaDeviceReport) -> impl IntoView {
    let hz =
        |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |value| format_hz(value as f64));
    let instrument = vec![
        line(
            "Board",
            report.board.clone().unwrap_or_else(|| "unnamed".to_owned()),
            true,
        ),
        line("Firmware", report.firmware.clone(), false),
        line("Port", report.port.clone(), false),
        line(
            "Battery",
            report.battery_mv.map_or_else(
                || "-".to_owned(),
                |mv| format!("{:.3} V", f64::from(mv) / 1000.0),
            ),
            false,
        ),
    ];
    let measurement = vec![
        line(
            "IF bandwidth",
            hz(report.bandwidth_hz.map(u64::from)),
            false,
        ),
        line("Drive level", describe_power(report.power), false),
        line(
            "Electrical delay",
            report
                .electrical_delay_s
                .map_or_else(|| "-".to_owned(), |delay| format_si(delay, "s", 3)),
            false,
        ),
        line(
            "S21 offset",
            report
                .s21_offset_db
                .map_or_else(|| "-".to_owned(), |db| format!("{db:.3} dB")),
            false,
        ),
    ];
    let range = vec![
        line("TCXO", hz(report.tcxo_hz), false),
        line("Harmonic above", hz(report.harmonic_threshold_hz), false),
        line(
            "Device sweep",
            report.sweep.map_or_else(
                || "-".to_owned(),
                |sweep| {
                    format!(
                        "{} \u{2013} {}",
                        format_hz(sweep.start_hz as f64),
                        format_hz(sweep.stop_hz as f64)
                    )
                },
            ),
            false,
        ),
        line(
            "Device points",
            report
                .sweep
                .map_or_else(|| "-".to_owned(), |sweep| sweep.points.to_string()),
            false,
        ),
    ];
    let calibration = if report.calibration.raw.is_empty() {
        "no calibration in memory".to_owned()
    } else {
        report.calibration.raw.clone()
    };
    let info = (!report.info.is_empty()).then(|| {
        let text = report.info.join("\n");
        view! {
            column(class = "tool-group") {
                text(class = "legend") {"Reported by the device"}
                text(class = "tool-pre") {{text}}
            }
        }
    });
    let commands = (!report.commands.is_empty()).then(|| {
        let title = format!("Shell commands ({})", report.commands.len());
        let text = report.commands.join(" ");
        view! {
            column(class = "tool-group") {
                text(class = "legend") {{title}}
                text(class = "tool-faint tool-mono") {{text}}
            }
        }
    });
    view! {
        column(class = "tool-stack") {
            box(class = "tool-groups") {
                {group("Instrument", instrument)}
                {group("Measurement", measurement)}
                {group("Reference and range", range)}
            }
            column(class = "tool-group") {
                text(class = "legend") {"Calibration"}
                text(class = "tool-mono tool-ink") {{calibration}}
            }
            {info}
            {commands}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_band_reads_as_its_edges_and_width() {
        let band = Band {
            start_hz: 14_000_000.0,
            stop_hz: 14_200_000.0,
            span_hz: 200_000.0,
            center_hz: 14_100_000.0,
            q: 70.5,
            truncated: false,
        };
        assert_eq!(describe_band(&band), "14 MHz \u{2013} 14.2 MHz (200 kHz)");
        let clipped = Band {
            truncated: true,
            ..band
        };
        assert_eq!(
            describe_band(&clipped),
            "14 MHz \u{2013} 14.2 MHz (\u{2265} 200 kHz, clipped)"
        );
    }

    #[test]
    fn the_top_drive_level_means_automatic() {
        assert_eq!(describe_power(Some(255)), "auto");
        assert_eq!(describe_power(Some(2)), "2");
        assert_eq!(describe_power(None), "-");
    }
}

use sdrmm_wire::tools::{NanoVnaComplex, NanoVnaSweep};

use super::{
    analysis::readouts,
    rf::{ComponentKind, REFERENCE_OHMS, complex, gain_db, magnitude, phase_deg},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Ri,
    Ma,
    Db,
}

impl Format {
    fn tag(self) -> &'static str {
        match self {
            Self::Ri => "RI",
            Self::Ma => "MA",
            Self::Db => "DB",
        }
    }
}

pub const FORMATS: [(Format, &str); 3] = [
    (Format::Ri, "Real / imaginary"),
    (Format::Ma, "Magnitude / angle"),
    (Format::Db, "dB / angle"),
];

#[must_use]
pub fn touchstone_s2p(sweep: &NanoVnaSweep, format: Format, recorded_at: Option<&str>) -> String {
    let zero = complex(0.0, 0.0);
    let mut lines = header(sweep, recorded_at);
    lines.push(
        "! S12 and S22 are not measured by this instrument and are written as zero.".to_owned(),
    );
    lines.push(format!("# Hz S {} R {REFERENCE_OHMS}", format.tag()));
    lines.push("! freq S11 S21 S12 S22".to_owned());
    lines.extend(sweep.points.iter().map(|point| {
        [
            point.frequency_hz.to_string(),
            pair(point.s11, format),
            pair(point.s21, format),
            pair(zero, format),
            pair(zero, format),
        ]
        .join(" ")
    }));
    lines.push(String::new());
    lines.join("\n")
}

#[must_use]
pub fn touchstone_s1p(sweep: &NanoVnaSweep, format: Format, recorded_at: Option<&str>) -> String {
    let mut lines = header(sweep, recorded_at);
    lines.push(format!("# Hz S {} R {REFERENCE_OHMS}", format.tag()));
    lines.push("! freq S11".to_owned());
    lines.extend(
        sweep
            .points
            .iter()
            .map(|point| format!("{} {}", point.frequency_hz, pair(point.s11, format))),
    );
    lines.push(String::new());
    lines.join("\n")
}

const CSV_COLUMNS: [&str; 21] = [
    "frequency_hz",
    "s11_real",
    "s11_imag",
    "s11_db",
    "s11_phase_deg",
    "vswr",
    "return_loss_db",
    "mismatch_loss_db",
    "resistance_ohm",
    "reactance_ohm",
    "impedance_magnitude_ohm",
    "q",
    "series_capacitance_f",
    "series_inductance_h",
    "conductance_s",
    "susceptance_s",
    "s21_real",
    "s21_imag",
    "s21_db",
    "s21_phase_deg",
    "group_delay_s",
];

#[must_use]
pub fn sweep_csv(sweep: &NanoVnaSweep) -> String {
    let mut lines = vec![CSV_COLUMNS.join(",")];
    lines.extend(readouts(&sweep.points).iter().map(|row| {
        let component = |kind| {
            row.component
                .filter(|component| component.kind == kind)
                .map(|component| component.value)
        };
        [
            Some(row.frequency_hz),
            Some(row.s11.re),
            Some(row.s11.im),
            Some(row.s11_db),
            Some(row.s11_phase_deg),
            Some(row.vswr),
            Some(row.return_loss_db),
            Some(row.mismatch_loss_db),
            row.impedance.map(|z| z.re),
            row.impedance.map(|z| z.im),
            Some(row.impedance_magnitude),
            Some(row.q),
            component(ComponentKind::Capacitance),
            component(ComponentKind::Inductance),
            row.admittance.map(|y| y.re),
            row.admittance.map(|y| y.im),
            Some(row.s21.re),
            Some(row.s21.im),
            Some(row.s21_db),
            Some(row.s21_phase_deg),
            Some(row.group_delay_s),
        ]
        .map(cell)
        .join(",")
    }));
    lines.push(String::new());
    lines.join("\n")
}

fn cell(value: Option<f64>) -> String {
    match value {
        None => String::new(),
        Some(value) if value.is_nan() => String::new(),
        Some(value) if value.is_infinite() => if value > 0.0 { "inf" } else { "-inf" }.to_owned(),
        Some(value) => format!("{value}"),
    }
}

fn header(sweep: &NanoVnaSweep, recorded_at: Option<&str>) -> Vec<String> {
    let device = &sweep.device;
    let calibration = if device.calibration.raw.is_empty() {
        "none"
    } else {
        device.calibration.raw.as_str()
    };
    let mut lines = vec!["! Measured with SDR-- (https://github.com/sdrminusminus)".to_owned()];
    if let Some(at) = recorded_at {
        lines.push(format!("! Recorded {at}"));
    }
    lines.push(format!(
        "! Instrument {} firmware {} on {}",
        device.board.as_deref().unwrap_or("NanoVNA"),
        device.firmware,
        device.port
    ));
    lines.push(format!(
        "! Points {} averages {}",
        sweep.points.len(),
        sweep.averages
    ));
    if let Some(bandwidth) = device.bandwidth_hz {
        lines.push(format!("! IF bandwidth {bandwidth} Hz"));
    }
    lines.push(format!("! Calibration {calibration}"));
    lines
}

fn pair(value: NanoVnaComplex, format: Format) -> String {
    match format {
        Format::Ri => format!("{} {}", fixed(value.re), fixed(value.im)),
        Format::Ma => format!("{} {}", fixed(magnitude(value)), angle(value)),
        Format::Db => {
            let db = gain_db(value);
            let shown = if db.is_finite() {
                format!("{db:.6}")
            } else {
                "-999.000000".to_owned()
            };
            format!("{shown} {}", angle(value))
        }
    }
}

fn angle(value: NanoVnaComplex) -> String {
    format!("{:.6}", unsigned_zero(phase_deg(value)))
}

fn unsigned_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn fixed(value: f64) -> String {
    if value.is_finite() {
        format!("{:.9}", unsigned_zero(value))
    } else {
        "0.000000000".to_owned()
    }
}

#[must_use]
pub fn export_filename(sweep: &NanoVnaSweep, extension: &str) -> String {
    let first = sweep.points.first().map_or(0, |point| point.frequency_hz);
    let last = sweep.points.last().map_or(0, |point| point.frequency_hz);
    let board = sweep
        .device
        .board
        .as_deref()
        .unwrap_or("nanovna")
        .to_lowercase();
    format!(
        "{}-{}-to-{}.{extension}",
        slug(&board),
        khz(first),
        khz(last)
    )
}

fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for character in text.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            out.push(character);
            gap = false;
        } else if !gap {
            out.push('-');
            gap = true;
        }
    }
    out
}

fn khz(hz: u64) -> String {
    format!("{}khz", (hz as f64 / 1000.0).round())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tools::nanovna::testdata::{point, sweep_of};

    fn sample() -> NanoVnaSweep {
        sweep_of(vec![
            point(1_000_000, complex(0.0, 0.5), complex(0.5, 0.0)),
            point(2_000_000, complex(0.25, -0.25), complex(0.9, 0.1)),
        ])
    }

    fn lines(text: &str) -> Vec<String> {
        text.trim().split('\n').map(str::to_owned).collect()
    }

    #[test]
    fn a_two_port_file_carries_the_option_line_and_zeroed_s12_s22() {
        let text = touchstone_s2p(&sample(), Format::Ri, None);
        let rows = lines(&text);
        assert!(rows.contains(&"# Hz S RI R 50".to_owned()));
        assert_eq!(
            rows[rows.len() - 2],
            "1000000 0.000000000 0.500000000 0.500000000 0.000000000 0.000000000 0.000000000 0.000000000 0.000000000"
        );
        assert!(text.contains(
            "! S12 and S22 are not measured by this instrument and are written as zero."
        ));
    }

    #[test]
    fn magnitude_and_db_formats_are_written_on_request() {
        let ma = lines(&touchstone_s2p(&sample(), Format::Ma, None));
        let fields: Vec<&str> = ma[ma.len() - 2].split(' ').collect();
        assert_eq!(fields[1..3], ["0.500000000", "90.000000"]);
        let db = lines(&touchstone_s2p(&sample(), Format::Db, None));
        assert_eq!(db[db.len() - 2].split(' ').nth(1), Some("-6.020600"));
    }

    #[test]
    fn a_one_port_file_holds_only_the_reflection() {
        let rows = lines(&touchstone_s1p(&sample(), Format::Ri, None));
        assert!(rows.contains(&"# Hz S RI R 50".to_owned()));
        assert_eq!(
            rows.last().map(String::as_str),
            Some("2000000 0.250000000 -0.250000000")
        );
    }

    #[test]
    fn the_header_names_the_instrument_and_its_calibration() {
        let header = touchstone_s2p(&sample(), Format::Ri, Some("2026-08-15T18:00:00Z"));
        assert!(header.contains("! Recorded 2026-08-15T18:00:00Z"));
        assert!(
            header.contains("! Instrument NanoVNA-H 4 firmware 1.2.46 on /dev/cu.usbmodem4001")
        );
        assert!(header.contains("! Calibration load isoln Es Er Et cal'ed"));
        assert!(header.contains("! IF bandwidth 1000 Hz"));
    }

    #[test]
    fn the_csv_heads_every_column_and_writes_a_row_per_point() {
        let rows = lines(&sweep_csv(&sample()));
        let head: Vec<&str> = rows[0].split(',').collect();
        assert!(head.contains(&"group_delay_s") && head.contains(&"series_inductance_h"));
        assert_eq!(rows.len(), 3);
        let first: Vec<&str> = rows[1].split(',').collect();
        assert_eq!(first[0], "1000000");
        assert!((first[5].parse::<f64>().unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn an_unmeasurable_cell_is_empty_and_an_infinite_one_is_inf() {
        let text = sweep_csv(&sweep_of(vec![point(
            1_000_000,
            complex(0.0, 0.0),
            complex(0.0, 0.0),
        )]));
        let rows = lines(&text);
        let head: Vec<&str> = rows[0].split(',').collect();
        let cells: Vec<&str> = rows[1].split(',').collect();
        let column = |name| head.iter().position(|entry| *entry == name).unwrap();
        assert_eq!(cells[column("return_loss_db")], "inf");
        assert_eq!(cells[column("series_capacitance_f")], "");
    }

    #[test]
    fn the_filename_names_the_instrument_and_the_range() {
        assert_eq!(
            export_filename(&sample(), "s2p"),
            "nanovna-h-4-1000khz-to-2000khz.s2p"
        );
    }
}

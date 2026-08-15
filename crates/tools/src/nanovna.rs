use std::{
    io::{self, Read, Write},
    sync::Arc,
    time::{Duration, Instant},
};

use sdrmm_wire::{
    MAX_NANOVNA_AVERAGES, MAX_NANOVNA_FREQ_HZ, MAX_NANOVNA_POINTS, MAX_NANOVNA_PORT_LEN,
    MIN_NANOVNA_FREQ_HZ, MIN_NANOVNA_POINTS, NANOVNA_TOOL_ID, NanoVnaComplex, NanoVnaDevice,
    NanoVnaPoint, NanoVnaRequest, NanoVnaResult, NanoVnaSweep, NanoVnaSweepRequest, ToolCategory,
    ToolDescriptor, ToolRequest, ToolResponse,
};

use crate::{Tool, ToolError};

const BAUD_RATE: u32 = 115_200;
const READ_TIMEOUT: Duration = Duration::from_millis(100);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const SEGMENT_POINTS: u32 = 101;

pub(crate) trait Connection: Read + Write + Send {}

impl<T: Read + Write + Send> Connection for T {}

pub(crate) trait Backend: Send + Sync {
    fn devices(&self) -> Result<Vec<NanoVnaDevice>, String>;
    fn connect(&self, port: &str) -> Result<Box<dyn Connection>, String>;
}

struct SystemBackend;

impl Backend for SystemBackend {
    fn devices(&self) -> Result<Vec<NanoVnaDevice>, String> {
        let mut devices: Vec<NanoVnaDevice> = serialport::available_ports()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(device_info)
            .collect();
        devices.sort_by(|left, right| {
            right
                .likely_nanovna
                .cmp(&left.likely_nanovna)
                .then_with(|| left.port.cmp(&right.port))
        });
        Ok(devices)
    }

    fn connect(&self, port: &str) -> Result<Box<dyn Connection>, String> {
        let connection = serialport::new(port, BAUD_RATE)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|error| error.to_string())?;
        connection
            .clear(serialport::ClearBuffer::Input)
            .map_err(|error| error.to_string())?;
        Ok(Box::new(connection))
    }
}

pub struct NanoVnaTool {
    backend: Arc<dyn Backend>,
}

impl Default for NanoVnaTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(SystemBackend),
        }
    }
}

#[cfg(test)]
impl NanoVnaTool {
    pub(crate) fn with_backend(backend: Arc<dyn Backend>) -> Self {
        Self { backend }
    }
}

impl Tool for NanoVnaTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: NANOVNA_TOOL_ID.to_owned(),
            name: "NanoVNA".to_owned(),
            summary: "Sweep S11 and S21 from a NanoVNA over USB serial".to_owned(),
            category: ToolCategory::Instrument,
            needs_hardware: true,
        }
    }

    fn run(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        let ToolRequest::NanoVna(request) = request else {
            return Err(ToolError::WrongTool {
                tool: NANOVNA_TOOL_ID,
                got: request.tool_id().to_owned(),
            });
        };
        match request {
            NanoVnaRequest::ListDevices => self
                .backend
                .devices()
                .map(|devices| ToolResponse::NanoVna(NanoVnaResult::Devices { devices }))
                .map_err(|reason| ToolError::Failed {
                    tool: NANOVNA_TOOL_ID,
                    reason: format!("listing serial ports: {reason}"),
                }),
            NanoVnaRequest::Sweep(request) => {
                validate(&request)?;
                let mut connection = self.backend.connect(&request.port).map_err(|reason| {
                    ToolError::Unavailable {
                        tool: NANOVNA_TOOL_ID,
                        reason: format!("cannot open {}: {reason}", request.port),
                    }
                })?;
                acquire(connection.as_mut(), &request)
                    .map(|sweep| ToolResponse::NanoVna(NanoVnaResult::Sweep(sweep)))
                    .map_err(|reason| ToolError::Failed {
                        tool: NANOVNA_TOOL_ID,
                        reason,
                    })
            }
        }
    }
}

fn device_info(info: serialport::SerialPortInfo) -> NanoVnaDevice {
    let port = info.port_name;
    match info.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            let likely_nanovna = matches!(
                (usb.vid, usb.pid),
                (0x0483, 0x5740) | (0x16c0, 0x0483) | (0x04b4, 0x0008)
            );
            let identity = usb
                .product
                .as_deref()
                .or(usb.manufacturer.as_deref())
                .unwrap_or("USB serial device");
            NanoVnaDevice {
                label: format!("{identity} · {port}"),
                port,
                likely_nanovna,
                serial_number: usb.serial_number,
                usb_vid: Some(usb.vid),
                usb_pid: Some(usb.pid),
            }
        }
        serialport::SerialPortType::BluetoothPort => NanoVnaDevice {
            label: format!("Bluetooth serial · {port}"),
            port,
            likely_nanovna: false,
            serial_number: None,
            usb_vid: None,
            usb_pid: None,
        },
        serialport::SerialPortType::PciPort => NanoVnaDevice {
            label: format!("PCI serial · {port}"),
            port,
            likely_nanovna: false,
            serial_number: None,
            usb_vid: None,
            usb_pid: None,
        },
        serialport::SerialPortType::Unknown => NanoVnaDevice {
            label: port.clone(),
            port,
            likely_nanovna: false,
            serial_number: None,
            usb_vid: None,
            usb_pid: None,
        },
    }
}

fn validate(request: &NanoVnaSweepRequest) -> Result<(), ToolError> {
    if request.port.is_empty()
        || request.port.len() > MAX_NANOVNA_PORT_LEN
        || request.port.contains('\0')
    {
        return Err(ToolError::Invalid(
            "port must name one serial device".to_owned(),
        ));
    }
    range(
        "start_hz",
        request.start_hz,
        MIN_NANOVNA_FREQ_HZ,
        MAX_NANOVNA_FREQ_HZ,
    )?;
    range(
        "stop_hz",
        request.stop_hz,
        MIN_NANOVNA_FREQ_HZ,
        MAX_NANOVNA_FREQ_HZ,
    )?;
    if request.stop_hz <= request.start_hz {
        return Err(ToolError::Invalid(
            "stop_hz must be greater than start_hz".to_owned(),
        ));
    }
    if !(MIN_NANOVNA_POINTS..=MAX_NANOVNA_POINTS).contains(&request.points) {
        return Err(ToolError::Invalid(format!(
            "points must be between {MIN_NANOVNA_POINTS} and {MAX_NANOVNA_POINTS}"
        )));
    }
    if !(1..=MAX_NANOVNA_AVERAGES).contains(&request.averages) {
        return Err(ToolError::Invalid(format!(
            "averages must be between 1 and {MAX_NANOVNA_AVERAGES}"
        )));
    }
    Ok(())
}

fn range(field: &str, value: u64, min: u64, max: u64) -> Result<(), ToolError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(ToolError::Invalid(format!(
        "{field} must be between {min} and {max}"
    )))
}

fn acquire(
    connection: &mut dyn Connection,
    request: &NanoVnaSweepRequest,
) -> Result<NanoVnaSweep, String> {
    let mut session = Session::new(connection);
    let firmware = session.command("version")?.join(" ");
    if firmware.is_empty() {
        return Err("the device returned no firmware version".to_owned());
    }
    let mut points = Vec::with_capacity(request.points as usize);
    let mut point_offset = 0;
    for size in segment_sizes(request.points) {
        let segment_start = frequency_at(request, point_offset);
        let segment_stop = frequency_at(request, point_offset + size - 1);
        let values = acquire_segment(
            &mut session,
            segment_start,
            segment_stop,
            size,
            request.averages,
        )?;
        points.extend(values);
        point_offset += size;
    }
    command_without_output(
        &mut session,
        &format!(
            "sweep {} {} {}",
            request.start_hz,
            request.stop_hz,
            request.points.min(SEGMENT_POINTS)
        ),
    )?;
    command_without_output(&mut session, "resume")?;
    if points.len() != request.points as usize {
        return Err(format!(
            "requested {} points but the device returned {}",
            request.points,
            points.len()
        ));
    }
    Ok(NanoVnaSweep {
        port: request.port.clone(),
        firmware,
        requested_points: request.points,
        averages: request.averages,
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
        command_without_output(session, &format!("scan {start_hz} {stop_hz} {points}"))?;
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

fn command_without_output(session: &mut Session<'_>, request: &str) -> Result<(), String> {
    let response = session.command(request)?;
    if response.is_empty() {
        return Ok(());
    }
    Err(format!(
        "device refused `{request}`: {}",
        response.join(" ")
    ))
}

struct Session<'a> {
    connection: &'a mut dyn Connection,
    pending: Vec<u8>,
}

impl<'a> Session<'a> {
    fn new(connection: &'a mut dyn Connection) -> Self {
        Self {
            connection,
            pending: Vec::new(),
        }
    }

    fn command(&mut self, request: &str) -> Result<Vec<String>, String> {
        self.connection
            .write_all(format!("{request}\r").as_bytes())
            .and_then(|()| self.connection.flush())
            .map_err(|error| format!("writing `{request}`: {error}"))?;
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let mut response = std::mem::take(&mut self.pending);
        let mut buffer = [0_u8; 512];
        let mut searched = 0;
        let prompt_at = loop {
            if let Some(position) = response[searched..]
                .windows(3)
                .position(|window| window == b"ch>")
            {
                break searched + position;
            }
            if response.len() >= MAX_RESPONSE_BYTES {
                return Err(format!("`{request}` exceeded the response limit"));
            }
            if Instant::now() >= deadline {
                return Err(format!("`{request}` timed out waiting for the prompt"));
            }
            searched = response.len().saturating_sub(2);
            match self.connection.read(&mut buffer) {
                Ok(0) => return Err(format!("`{request}` ended before the prompt")),
                Ok(read) => response.extend_from_slice(&buffer[..read]),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(format!("reading `{request}`: {error}")),
            }
        };
        self.pending = response.split_off(prompt_at + 3);
        response.truncate(prompt_at);
        let text = std::str::from_utf8(&response)
            .map_err(|error| format!("`{request}` returned non-UTF-8 data: {error}"))?;
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && *line != request)
            .map(str::to_owned)
            .collect())
    }
}

fn segment_sizes(points: u32) -> Vec<u32> {
    let segment_count = points.div_ceil(SEGMENT_POINTS);
    let base_size = points / segment_count;
    let extra = points % segment_count;
    (0..segment_count)
        .map(|segment| base_size + u32::from(segment < extra))
        .collect()
}

fn parse_frequencies(lines: Vec<String>) -> Result<Vec<u64>, String> {
    lines
        .into_iter()
        .map(|line| {
            line.split_whitespace()
                .next()
                .ok_or_else(|| "empty frequency row".to_owned())?
                .parse::<u64>()
                .map_err(|error| format!("invalid frequency row `{line}`: {error}"))
        })
        .collect()
}

fn parse_complex(lines: Vec<String>) -> Result<Vec<NanoVnaComplex>, String> {
    lines
        .into_iter()
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() != 2 {
                return Err(format!("invalid complex row `{line}`"));
            }
            let re = fields[0]
                .parse::<f64>()
                .map_err(|error| format!("invalid complex row `{line}`: {error}"))?;
            let im = fields[1]
                .parse::<f64>()
                .map_err(|error| format!("invalid complex row `{line}`: {error}"))?;
            if !re.is_finite() || !im.is_finite() {
                return Err(format!("non-finite complex row `{line}`"));
            }
            Ok(NanoVnaComplex { re, im })
        })
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
    use std::collections::VecDeque;

    use super::*;

    struct FixtureConnection {
        reads: VecDeque<u8>,
        writes: Vec<u8>,
    }

    impl Read for FixtureConnection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = buffer.len().min(self.reads.len()).min(17);
            for slot in &mut buffer[..read] {
                *slot = self.reads.pop_front().unwrap_or_default();
            }
            Ok(read)
        }
    }

    impl Write for FixtureConnection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn request() -> NanoVnaSweepRequest {
        NanoVnaSweepRequest {
            port: "/dev/ttyACM0".to_owned(),
            start_hz: 1_000_000,
            stop_hz: 2_000_000,
            points: 11,
            averages: 1,
        }
    }

    fn fixture() -> String {
        let frequencies = (0..11)
            .map(|index| (1_000_000 + index * 100_000).to_string())
            .collect::<Vec<_>>()
            .join("\r\n");
        let s11 = (0..11)
            .map(|index| format!("{} {}", 0.1 + f64::from(index) / 100.0, -0.2))
            .collect::<Vec<_>>()
            .join("\r\n");
        let s21 = (0..11)
            .map(|index| format!("{} {}", 0.8 - f64::from(index) / 100.0, 0.1))
            .collect::<Vec<_>>()
            .join("\r\n");
        format!(
            "version\r\n0.7.2\r\nch> scan 1000000 2000000 11\r\nch> frequencies\r\n{frequencies}\r\nch> data 0\r\n{s11}\r\nch> data 1\r\n{s21}\r\nch> sweep 1000000 2000000 11\r\nch> resume\r\nch> "
        )
    }

    #[test]
    fn recorded_shell_fixture_produces_a_sweep() {
        let mut connection = FixtureConnection {
            reads: fixture().bytes().collect(),
            writes: Vec::new(),
        };
        let sweep = acquire(&mut connection, &request()).expect("acquire fixture");
        assert_eq!(sweep.firmware, "0.7.2");
        assert_eq!(sweep.points.len(), 11);
        assert_eq!(sweep.points[0].frequency_hz, 1_000_000);
        assert_eq!(sweep.points[10].frequency_hz, 2_000_000);
        assert!((sweep.points[0].s11.re - 0.1).abs() < f64::EPSILON);
        assert!((sweep.points[10].s21.re - 0.7).abs() < f64::EPSILON);
        let commands = String::from_utf8(connection.writes).expect("ASCII commands");
        assert_eq!(
            commands,
            "version\rscan 1000000 2000000 11\rfrequencies\rdata 0\rdata 1\rsweep 1000000 2000000 11\rresume\r"
        );
    }

    #[test]
    fn response_parser_handles_a_prompt_split_across_reads() {
        let mut connection = FixtureConnection {
            reads: b"version\r\nNanoVNA-H4\r\nch> ".iter().copied().collect(),
            writes: Vec::new(),
        };
        let mut session = Session::new(&mut connection);
        assert_eq!(
            session.command("version"),
            Ok(vec!["NanoVNA-H4".to_owned()])
        );
    }

    #[test]
    fn unsupported_scan_is_reported_before_data_is_read() {
        let mut connection = FixtureConnection {
            reads: b"version\r\n0.1.0\r\nch> scan 1000000 2000000 11\r\n?\r\nch> "
                .iter()
                .copied()
                .collect(),
            writes: Vec::new(),
        };
        let error = acquire(&mut connection, &request()).expect_err("scan must be supported");
        assert!(error.contains("refused `scan"), "{error}");
        assert_eq!(
            String::from_utf8(connection.writes).expect("ASCII commands"),
            "version\rscan 1000000 2000000 11\r"
        );
    }

    #[test]
    fn validation_rejects_unsafe_or_impossible_sweeps() {
        let mut invalid = request();
        invalid.port = "bad\0port".to_owned();
        assert!(
            matches!(validate(&invalid), Err(ToolError::Invalid(reason)) if reason.contains("port"))
        );
        invalid = request();
        invalid.stop_hz = invalid.start_hz;
        assert!(
            matches!(validate(&invalid), Err(ToolError::Invalid(reason)) if reason.contains("stop_hz"))
        );
        invalid = request();
        invalid.points = MAX_NANOVNA_POINTS + 1;
        assert!(
            matches!(validate(&invalid), Err(ToolError::Invalid(reason)) if reason.contains("points"))
        );
    }

    #[test]
    fn segment_distribution_keeps_every_chunk_within_device_limits() {
        let sizes = segment_sizes(102);
        assert_eq!(sizes, vec![51, 51]);
        assert!(sizes.into_iter().all(|size| size <= SEGMENT_POINTS));
    }

    #[test]
    fn complex_rows_reject_non_finite_values() {
        assert!(parse_complex(vec!["NaN 0".to_owned()]).is_err());
        assert!(parse_complex(vec!["0 inf".to_owned()]).is_err());
        assert!(parse_complex(vec!["0 1 2".to_owned()]).is_err());
    }
}

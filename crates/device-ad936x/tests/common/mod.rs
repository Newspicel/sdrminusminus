#![allow(clippy::expect_used)]
use std::{
    collections::HashMap,
    io::{Read as _, Write as _},
    net::TcpStream,
    sync::{Arc, Mutex},
};

use sdrmm_device::{lock, net::testing::FakeServer};

/// What one lane of the receive buffer carries, as a repeating run of counts. The driver turns
/// these into samples, so a test can name the values it expects back.
pub const RAMP: [i16; 8] = [0, 1, -1, 2047, -2048, 100, -100, 7];

#[derive(Default)]
pub struct Recorded {
    pub commands: Vec<String>,
    pub opened: Vec<String>,
    pub transmitted: Vec<u8>,
}

pub struct FakeIiod {
    server: FakeServer,
    state: Arc<State>,
}

struct State {
    xml: String,
    attributes: Mutex<HashMap<String, String>>,
    recorded: Mutex<Recorded>,
    refuse: Mutex<Option<(String, i32)>>,
}

impl FakeIiod {
    pub fn spawn(lanes: usize) -> Self {
        Self::with(context_xml(lanes), attributes(lanes))
    }

    pub fn with(xml: String, attributes: HashMap<String, String>) -> Self {
        let state = Arc::new(State {
            xml,
            attributes: Mutex::new(attributes),
            recorded: Mutex::new(Recorded::default()),
            refuse: Mutex::new(None),
        });
        let served = state.clone();
        let server = FakeServer::spawn(move |stream, _| serve(&served, stream));
        Self { server, state }
    }

    pub fn endpoint(&self) -> String {
        self.server.endpoint()
    }

    pub fn connections(&self) -> usize {
        self.server.connections()
    }

    pub fn attribute(&self, key: &str) -> Option<String> {
        lock(&self.state.attributes).get(key).cloned()
    }

    pub fn commands(&self) -> Vec<String> {
        lock(&self.state.recorded).commands.clone()
    }

    pub fn opened(&self) -> Vec<String> {
        lock(&self.state.recorded).opened.clone()
    }

    pub fn transmitted(&self) -> Vec<u8> {
        lock(&self.state.recorded).transmitted.clone()
    }

    /// Makes the radio refuse every command that starts with `prefix`, a bare command word or
    /// one naming the attribute, the way a real one answers when the hardware will not take a
    /// setting.
    pub fn refuse(&self, prefix: &str, errno: i32) {
        *lock(&self.state.refuse) = Some((prefix.to_string(), errno));
    }
}

fn serve(state: &Arc<State>, mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    while let Some(line) = read_line(&mut stream) {
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        lock(&state.recorded).commands.push(trimmed.clone());
        if handle(state, &mut stream, &trimmed).is_none() {
            return;
        }
    }
}

fn handle(state: &Arc<State>, stream: &mut TcpStream, line: &str) -> Option<()> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let command = *words.first()?;
    if let Some((refused, errno)) = lock(&state.refuse).clone()
        && line.starts_with(&refused)
    {
        // A refused write still takes its payload, or the conversation is left desynchronised.
        if command == "WRITE"
            && let Some(count) = words.last().and_then(|n| n.parse::<usize>().ok())
        {
            let mut payload = vec![0u8; count];
            stream.read_exact(&mut payload).ok()?;
        }
        return say(stream, &format!("{errno}\n"));
    }
    match command {
        "VERSION" => say(stream, "1.1.fakeiio\n"),
        "PRINT" => say(stream, &format!("{}\n{}\n", state.xml.len(), state.xml)),
        "TIMEOUT" | "CLOSE" => say(stream, "0\n"),
        "OPEN" => {
            lock(&state.recorded).opened.push(line.to_string());
            say(stream, "0\n")
        }
        "READ" => read_attribute(state, stream, &words),
        "WRITE" => write_attribute(state, stream, &words),
        "READBUF" => read_buffer(stream, &words),
        "WRITEBUF" => write_buffer(state, stream, &words),
        _ => say(stream, "-22\n"),
    }
}

fn read_attribute(state: &Arc<State>, stream: &mut TcpStream, words: &[&str]) -> Option<()> {
    let key = attribute_key(words)?;
    match lock(&state.attributes).get(&key) {
        Some(value) => {
            let payload = format!("{value}\0");
            say(stream, &format!("{}\n{payload}\n", payload.len()))
        }
        None => say(stream, "-2\n"),
    }
}

fn write_attribute(state: &Arc<State>, stream: &mut TcpStream, words: &[&str]) -> Option<()> {
    let count: usize = words.last()?.parse().ok()?;
    let key = attribute_key(&words[..words.len() - 1])?;
    let mut payload = vec![0u8; count];
    stream.read_exact(&mut payload).ok()?;
    let value = String::from_utf8_lossy(&payload)
        .trim_end_matches('\0')
        .to_string();
    let mut attributes = lock(&state.attributes);
    if !store(&mut attributes, &key, &value) {
        return say(stream, "-22\n");
    }
    say(stream, &format!("{count}\n"))
}

const SHARED: [&str; 5] = [
    "rf_bandwidth",
    "sampling_frequency",
    "quadrature_tracking_en",
    "rf_dc_offset_tracking_en",
    "bb_dc_offset_tracking_en",
];

fn store(attributes: &mut HashMap<String, String>, key: &str, value: &str) -> bool {
    let parts: Vec<&str> = key.split('/').collect();
    let [device, way, channel, attr] = parts[..] else {
        attributes.insert(key.to_string(), value.to_string());
        return true;
    };
    if device != "ad9361-phy" {
        attributes.insert(key.to_string(), value.to_string());
        return true;
    }
    match (way, channel, attr) {
        (_, "voltage0", "rf_port_select") => {
            same_on_every_lane(attributes, way, attr, value);
            true
        }
        (_, _, "rf_port_select") => false,
        ("INPUT", _, "hardwaregain") => set_gain(attributes, key, channel, value),
        ("OUTPUT", "altvoltage0", "frequency") => {
            attributes.insert(key.to_string(), value.to_string());
            follow_band(attributes, value.parse().unwrap_or(0.0));
            true
        }
        (_, _, attr) if SHARED.contains(&attr) => {
            same_on_every_lane(attributes, way, attr, value);
            if attr == "sampling_frequency" {
                converters_follow(attributes, value);
            }
            true
        }
        _ => {
            attributes.insert(key.to_string(), value.to_string());
            true
        }
    }
}

fn same_on_every_lane(
    attributes: &mut HashMap<String, String>,
    way: &str,
    attr: &str,
    value: &str,
) {
    let prefix = format!("ad9361-phy/{way}/");
    let keys: Vec<String> = attributes
        .keys()
        .filter(|key| key.starts_with(&prefix) && key.ends_with(&format!("/{attr}")))
        .cloned()
        .collect();
    for key in keys {
        attributes.insert(key, value.to_string());
    }
}

fn gain_limit(attributes: &HashMap<String, String>, channel: &str) -> f64 {
    attributes
        .get(&format!(
            "ad9361-phy/INPUT/{channel}/hardwaregain_available"
        ))
        .and_then(|range| {
            range
                .trim_matches(['[', ']'])
                .split_whitespace()
                .last()?
                .parse()
                .ok()
        })
        .unwrap_or(f64::MAX)
}

fn set_gain(
    attributes: &mut HashMap<String, String>,
    key: &str,
    channel: &str,
    value: &str,
) -> bool {
    let wanted: f64 = value.parse().unwrap_or(f64::MAX);
    if wanted > gain_limit(attributes, channel) {
        return false;
    }
    attributes.insert(key.to_string(), value.to_string());
    true
}

fn follow_band(attributes: &mut HashMap<String, String>, hz: f64) {
    let range = if hz < 1.3e9 {
        "[-1 1 73]"
    } else if hz < 4e9 {
        "[-3 1 71]"
    } else {
        "[-10 1 62]"
    };
    same_on_every_lane(attributes, "INPUT", "hardwaregain_available", range);
    let channels: Vec<String> = attributes
        .keys()
        .filter_map(|key| {
            key.strip_prefix("ad9361-phy/INPUT/")?
                .strip_suffix("/hardwaregain")
        })
        .map(str::to_string)
        .collect();
    for channel in channels {
        let limit = gain_limit(attributes, &channel);
        let key = format!("ad9361-phy/INPUT/{channel}/hardwaregain");
        let held: f64 = attributes
            .get(&key)
            .and_then(|gain| gain.split_whitespace().next()?.parse().ok())
            .unwrap_or(0.0);
        if held > limit {
            attributes.insert(key, format!("{limit:.6} dB"));
        }
    }
}

fn converters_follow(attributes: &mut HashMap<String, String>, value: &str) {
    let rate: f64 = value.parse().unwrap_or(0.0);
    let offered = format!("{rate:.0} {:.0}", (rate / 8.0).floor());
    for (device, way) in [
        ("cf-ad9361-lpc", "INPUT"),
        ("cf-ad9361-dds-core-lpc", "OUTPUT"),
    ] {
        let base = format!("{device}/{way}/voltage0");
        attributes.insert(format!("{base}/sampling_frequency"), format!("{rate:.0}"));
        attributes.insert(
            format!("{base}/sampling_frequency_available"),
            offered.clone(),
        );
    }
}

/// `READ dev attr` and `READ dev INPUT chan attr` name the same thing in this store.
fn attribute_key(words: &[&str]) -> Option<String> {
    let device = words.get(1)?;
    match words.get(2) {
        Some(&"DEBUG") => Some(format!("{device}/DEBUG/{}", words.get(3)?)),
        Some(way @ (&"INPUT" | &"OUTPUT")) => Some(format!(
            "{device}/{way}/{}/{}",
            words.get(3)?,
            words.get(4)?
        )),
        Some(attr) => Some(format!("{device}//{attr}")),
        None => None,
    }
}

fn read_buffer(stream: &mut TcpStream, words: &[&str]) -> Option<()> {
    let count: usize = words.get(2)?.parse().ok()?;
    say(stream, &format!("{count}\n"))?;
    say(stream, "00000003\n")?;
    let pattern: Vec<u8> = RAMP.iter().flat_map(|v| v.to_le_bytes()).collect();
    let block: Vec<u8> = pattern.iter().copied().cycle().take(count).collect();
    stream.write_all(&block).ok()?;
    Some(())
}

fn write_buffer(state: &Arc<State>, stream: &mut TcpStream, words: &[&str]) -> Option<()> {
    let count: usize = words.get(2)?.parse().ok()?;
    say(stream, "0\n")?;
    let mut payload = vec![0u8; count];
    stream.read_exact(&mut payload).ok()?;
    lock(&state.recorded)
        .transmitted
        .extend_from_slice(&payload);
    say(stream, &format!("{count}\n"))
}

fn say(stream: &mut TcpStream, text: &str) -> Option<()> {
    stream.write_all(text.as_bytes()).ok()
}

fn read_line(stream: &mut TcpStream) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        if byte[0] == b'\n' {
            return Some(String::from_utf8_lossy(&line).to_string());
        }
        line.push(byte[0]);
    }
}

/// An AD936x context with `lanes` receive and transmit lanes, as iiod describes one.
pub fn context_xml(lanes: usize) -> String {
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="utf-8"?>
<context name="network" description="192.168.1.10 Linux (none) 4.19.0 armv7l" >
<context-attribute name="hw_model" value="Analog Devices ANTSDR Rev.C (Z7020-AD9361)" />
<context-attribute name="hw_serial" value="1044734c960500111e002e0041984fc267" />
<device id="iio:device0" name="ad9361-phy" >
<channel id="altvoltage0" name="RX_LO" type="output" >
<attribute name="frequency" filename="out_altvoltage0_RX_LO_frequency" value="2400000000" />
</channel>
<channel id="altvoltage1" name="TX_LO" type="output" >
<attribute name="frequency" filename="out_altvoltage1_TX_LO_frequency" value="2450000000" />
</channel>
<attribute name="ensm_mode" value="fdd" />
<attribute name="xo_correction" value="40000000" />
"#,
    );
    for lane in 0..lanes {
        xml.push_str(&format!(
            r#"<channel id="voltage{lane}" type="input" >
<attribute name="hardwaregain" filename="in_voltage{lane}_hardwaregain" value="40.000000 dB" />
<attribute name="gain_control_mode" filename="in_voltage{lane}_gain_control_mode" value="manual" />
<attribute name="rf_port_select" filename="in_voltage{lane}_rf_port_select" value="A_BALANCED" />
<attribute name="rf_bandwidth" filename="in_voltage_rf_bandwidth" value="18000000" />
<attribute name="sampling_frequency" filename="in_voltage_sampling_frequency" value="2400000" />
<attribute name="quadrature_tracking_en" filename="in_voltage_quadrature_tracking_en" value="1" />
<attribute name="rf_dc_offset_tracking_en" filename="in_voltage_rf_dc_offset_tracking_en" value="1" />
<attribute name="bb_dc_offset_tracking_en" filename="in_voltage_bb_dc_offset_tracking_en" value="1" />
<attribute name="filter_fir_en" filename="in_voltage_filter_fir_en" value="0" />
</channel>
<channel id="voltage{lane}" type="output" >
<attribute name="hardwaregain" filename="out_voltage{lane}_hardwaregain" value="-10.000000 dB" />
<attribute name="rf_port_select" filename="out_voltage{lane}_rf_port_select" value="A" />
<attribute name="rf_bandwidth" filename="out_voltage_rf_bandwidth" value="18000000" />
</channel>
"#
        ));
    }
    xml.push_str("</device>\n");
    xml.push_str(&buffer_device(
        "iio:device3",
        "cf-ad9361-lpc",
        "input",
        "le:S12/16&gt;&gt;0",
        lanes * 2,
    ));
    xml.push_str(&buffer_device(
        "iio:device2",
        "cf-ad9361-dds-core-lpc",
        "output",
        "le:S16/16&gt;&gt;0",
        lanes * 2,
    ));
    xml.push_str("</context>");
    xml
}

fn buffer_device(id: &str, name: &str, way: &str, format: &str, channels: usize) -> String {
    let mut xml = format!("<device id=\"{id}\" name=\"{name}\" >\n");
    for index in 0..channels {
        xml.push_str(&format!(
            "<channel id=\"voltage{index}\" type=\"{way}\" >\
             <scan-element index=\"{index}\" format=\"{format}\" />\
             <attribute name=\"sampling_frequency\" value=\"2400000\" /></channel>\n"
        ));
    }
    xml.push_str("</device>\n");
    xml
}

/// The values a radio answers a bounded attribute with, so the driver reads its limits off the
/// wire rather than falling back to what any AD936x can do.
pub fn attributes(lanes: usize) -> HashMap<String, String> {
    let mut attributes = HashMap::new();
    let mut put = |key: &str, value: &str| {
        attributes.insert(key.to_string(), value.to_string());
    };
    put("ad9361-phy/OUTPUT/altvoltage0/frequency", "2400000000");
    put(
        "ad9361-phy/OUTPUT/altvoltage0/frequency_available",
        "[70000000 1 6000000000]",
    );
    put("ad9361-phy/OUTPUT/altvoltage1/frequency", "2400000000");
    put("ad9361-phy//xo_correction", "40000000");
    for base in [
        "cf-ad9361-lpc/INPUT/voltage0",
        "cf-ad9361-dds-core-lpc/OUTPUT/voltage0",
    ] {
        put(&format!("{base}/sampling_frequency"), "2400000");
        put(
            &format!("{base}/sampling_frequency_available"),
            "2400000 300000",
        );
    }
    put(
        "ad9361-phy//xo_correction_available",
        "[39992159 1 40008159]",
    );
    for lane in 0..lanes {
        let rx = format!("ad9361-phy/INPUT/voltage{lane}");
        let tx = format!("ad9361-phy/OUTPUT/voltage{lane}");
        put(&format!("{rx}/hardwaregain"), "40.000000 dB");
        put(&format!("{rx}/hardwaregain_available"), "[-3 1 71]");
        put(&format!("{rx}/gain_control_mode"), "manual");
        put(
            &format!("{rx}/gain_control_mode_available"),
            "manual fast_attack slow_attack hybrid",
        );
        put(&format!("{rx}/rf_port_select"), "A_BALANCED");
        put(
            &format!("{rx}/rf_port_select_available"),
            "A_BALANCED B_BALANCED TX_MONITOR1",
        );
        put(&format!("{rx}/rf_bandwidth"), "18000000");
        put(
            &format!("{rx}/rf_bandwidth_available"),
            "[200000 1 56000000]",
        );
        put(&format!("{rx}/sampling_frequency"), "2400000");
        put(
            &format!("{rx}/sampling_frequency_available"),
            "[2083333 1 61440000]",
        );
        put(&format!("{rx}/quadrature_tracking_en"), "1");
        put(&format!("{rx}/rf_dc_offset_tracking_en"), "1");
        put(&format!("{rx}/bb_dc_offset_tracking_en"), "1");
        put(&format!("{rx}/filter_fir_en"), "0");
        put(&format!("{tx}/hardwaregain"), "-10.000000 dB");
        put(
            &format!("{tx}/hardwaregain_available"),
            "[-89.750000 0.250000 0.000000]",
        );
        put(&format!("{tx}/rf_port_select"), "A");
        put(&format!("{tx}/rf_port_select_available"), "A B");
        put(&format!("{tx}/rf_bandwidth"), "18000000");
        put(
            &format!("{tx}/rf_bandwidth_available"),
            "[200000 1 40000000]",
        );
    }
    attributes
}

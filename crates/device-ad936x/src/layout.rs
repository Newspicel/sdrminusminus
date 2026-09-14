use sdrmm_device::DeviceError;

use crate::iio::{Channel, Context, Device, Format};

pub(crate) const RX_LO: &str = "altvoltage0";
pub(crate) const TX_LO: &str = "altvoltage1";

pub(crate) const FREQUENCY: &str = "frequency";
pub(crate) const HARDWAREGAIN: &str = "hardwaregain";
pub(crate) const GAIN_CONTROL_MODE: &str = "gain_control_mode";
pub(crate) const RF_BANDWIDTH: &str = "rf_bandwidth";
pub(crate) const RF_PORT_SELECT: &str = "rf_port_select";
pub(crate) const SAMPLING_FREQUENCY: &str = "sampling_frequency";
pub(crate) const QUADRATURE_TRACKING: &str = "quadrature_tracking_en";
pub(crate) const RF_DC_TRACKING: &str = "rf_dc_offset_tracking_en";
pub(crate) const BB_DC_TRACKING: &str = "bb_dc_offset_tracking_en";
pub(crate) const FILTER_FIR_EN: &str = "filter_fir_en";
pub(crate) const XO_CORRECTION: &str = "xo_correction";
pub(crate) const AVAILABLE: &str = "_available";

/// The nominal crystal an AD936x board runs, against which a correction becomes a part per
/// million. Read from the radio when it says so, because a board may be built around another.
pub(crate) const NOMINAL_XO_HZ: f64 = 40_000_000.0;

/// What one radio's IIO context actually carries: which device is the transceiver, which devices
/// carry the sample buffers, and how many lanes each of those has.
///
/// Everything here is read from the radio rather than assumed, so a 1R1T board and a 2R2T board
/// are the same code path and a board with the other AD936x part reports its own limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub(crate) phy: String,
    pub(crate) rx: Option<Stream>,
    pub(crate) tx: Option<Stream>,
    pub(crate) rx_ports: Vec<String>,
    pub(crate) tx_ports: Vec<String>,
}

/// One direction's sample buffer: the device that carries it and the shape of its scan elements.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stream {
    pub(crate) device: String,
    pub(crate) channels: Vec<u32>,
    pub(crate) format: Format,
    pub(crate) scan_total: usize,
}

impl Stream {
    /// Lanes are an in-phase and a quadrature scan element each, so a 2×2 radio shows four.
    pub(crate) fn lanes(&self) -> usize {
        self.channels.len() / 2
    }

    /// The scan elements of the first `lanes` lanes, which is what the enabled mask is built from.
    pub(crate) fn elements(&self, lanes: usize) -> Vec<u32> {
        self.channels
            .iter()
            .copied()
            .take(lanes.saturating_mul(2))
            .collect()
    }

    pub(crate) const fn sample_bytes(&self, lanes: usize) -> usize {
        lanes * 2 * self.format.storage_bytes()
    }
}

impl Layout {
    pub(crate) fn read(context: &Context) -> Result<Self, DeviceError> {
        let phy = context
            .devices
            .iter()
            .find(|device| is_phy(device))
            .ok_or_else(|| {
                DeviceError::Unsupported(format!(
                    "this radio serves iiod but carries no AD936x transceiver: it has {}",
                    device_names(context)
                ))
            })?;
        let layout = Self {
            phy: phy.name.clone(),
            rx: stream(context, false),
            tx: stream(context, true),
            rx_ports: ports(phy, false),
            tx_ports: ports(phy, true),
        };
        if layout.rx_streams() == 0 {
            return Err(DeviceError::Unsupported(format!(
                "this radio's AD936x has no receive buffer to read: it has {}",
                device_names(context)
            )));
        }
        Ok(layout)
    }

    pub(crate) fn rx_streams(&self) -> usize {
        lanes(self.rx.as_ref(), &self.rx_ports)
    }

    pub(crate) fn tx_streams(&self) -> usize {
        lanes(self.tx.as_ref(), &self.tx_ports)
    }

    /// The transceiver channel that carries one lane's gain, port and squelch settings.
    pub(crate) fn port(&self, output: bool, stream: usize) -> Option<&str> {
        let ports = if output {
            &self.tx_ports
        } else {
            &self.rx_ports
        };
        ports.get(stream).map(String::as_str)
    }
}

fn lanes(stream: Option<&Stream>, ports: &[String]) -> usize {
    stream.map_or(0, Stream::lanes).min(ports.len())
}

/// The transceiver itself: the one device carrying a tunable local oscillator.
fn is_phy(device: &Device) -> bool {
    device
        .channel(RX_LO, true)
        .is_some_and(|channel| channel.has(FREQUENCY))
}

/// A buffer device of one direction: the converter interface, which is the only device whose
/// channels carry scan elements in that direction.
fn stream(context: &Context, output: bool) -> Option<Stream> {
    context.devices.iter().find_map(|device| {
        let scan = device.scan_channels(output);
        let format = scan.first()?.scan?.format;
        let channels: Vec<u32> = scan
            .iter()
            .filter_map(|c| c.scan.map(|s| s.index))
            .collect();
        (channels.len() >= 2).then(|| Stream {
            device: device.name.clone(),
            scan_total: channels.len(),
            channels,
            format,
        })
    })
}

/// The transceiver channels that carry a gain, in lane order. The aux converters share the
/// `voltage` naming and are told apart by having no gain of their own.
fn ports(phy: &Device, output: bool) -> Vec<String> {
    let mut found: Vec<&Channel> = phy
        .channels
        .iter()
        .filter(|channel| {
            channel.output == output
                && channel.id.starts_with("voltage")
                && channel.has(HARDWAREGAIN)
        })
        .collect();
    found.sort_by_key(|channel| suffix(&channel.id));
    found.iter().map(|channel| channel.id.clone()).collect()
}

fn suffix(id: &str) -> u32 {
    id.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(u32::MAX)
}

fn device_names(context: &Context) -> String {
    context
        .devices
        .iter()
        .map(|device| device.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name of the list an attribute publishes its permitted settings under.
pub(crate) fn available(attr: &str) -> String {
    format!("{attr}{AVAILABLE}")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn context(xml: &str) -> Context {
        Context::parse(xml).expect("a context")
    }

    fn two_by_two() -> Context {
        context(
            r#"<context name="n" >
<device id="iio:device0" name="ad9361-phy" >
<channel id="altvoltage0" name="RX_LO" type="output" >
<attribute name="frequency" value="2400000000" />
<attribute name="frequency_available" value="[70000000 1 6000000000]" />
</channel>
<channel id="altvoltage1" name="TX_LO" type="output" >
<attribute name="frequency" value="2450000000" />
</channel>
<channel id="voltage1" type="input" >
<attribute name="hardwaregain" value="30.000000 dB" />
</channel>
<channel id="voltage0" type="input" >
<attribute name="hardwaregain" value="71.000000 dB" />
<attribute name="rf_bandwidth" value="18000000" />
</channel>
<channel id="voltage2" type="input" >
<attribute name="raw" value="975" />
</channel>
<channel id="voltage0" type="output" >
<attribute name="hardwaregain" value="-10.000000 dB" />
</channel>
<channel id="voltage1" type="output" >
<attribute name="hardwaregain" value="-10.000000 dB" />
</channel>
<channel id="voltage3" type="output" >
<attribute name="raw" value="306" />
</channel>
</device>
<device id="iio:device2" name="cf-ad9361-dds-core-lpc" >
<channel id="voltage0" type="output" ><scan-element index="0" format="le:S16/16&gt;&gt;0" /></channel>
<channel id="voltage1" type="output" ><scan-element index="1" format="le:S16/16&gt;&gt;0" /></channel>
<channel id="voltage2" type="output" ><scan-element index="2" format="le:S16/16&gt;&gt;0" /></channel>
<channel id="voltage3" type="output" ><scan-element index="3" format="le:S16/16&gt;&gt;0" /></channel>
</device>
<device id="iio:device3" name="cf-ad9361-lpc" >
<channel id="voltage0" type="input" ><scan-element index="0" format="le:S12/16&gt;&gt;0" /></channel>
<channel id="voltage1" type="input" ><scan-element index="1" format="le:S12/16&gt;&gt;0" /></channel>
<channel id="voltage2" type="input" ><scan-element index="2" format="le:S12/16&gt;&gt;0" /></channel>
<channel id="voltage3" type="input" ><scan-element index="3" format="le:S12/16&gt;&gt;0" /></channel>
</device>
</context>"#,
        )
    }

    fn one_by_one() -> Context {
        context(
            r#"<context name="n" >
<device id="iio:device0" name="ad9361-phy" >
<channel id="altvoltage0" name="RX_LO" type="output" >
<attribute name="frequency" value="100000000" />
</channel>
<channel id="voltage0" type="input" >
<attribute name="hardwaregain" value="40.000000 dB" />
</channel>
</device>
<device id="iio:device3" name="cf-ad9361-lpc" >
<channel id="voltage0" type="input" ><scan-element index="0" format="le:S12/16&gt;&gt;0" /></channel>
<channel id="voltage1" type="input" ><scan-element index="1" format="le:S12/16&gt;&gt;0" /></channel>
</device>
</context>"#,
        )
    }

    pub(crate) fn two_by_two_layout() -> Layout {
        Layout::read(&two_by_two()).expect("layout")
    }

    pub(crate) fn one_by_one_layout() -> Layout {
        Layout::read(&one_by_one()).expect("layout")
    }

    #[test]
    fn a_two_by_two_radio_reports_two_lanes_in_each_direction() {
        let layout = Layout::read(&two_by_two()).expect("layout");
        assert_eq!(layout.phy, "ad9361-phy");
        assert_eq!(layout.rx_streams(), 2);
        assert_eq!(layout.tx_streams(), 2);
        assert_eq!(layout.rx_ports, vec!["voltage0", "voltage1"]);
        assert_eq!(layout.tx_ports, vec!["voltage0", "voltage1"]);
        assert_eq!(layout.port(false, 1), Some("voltage1"));
        assert_eq!(layout.port(true, 2), None);
    }

    #[test]
    fn the_aux_converters_are_not_mistaken_for_lanes() {
        let layout = Layout::read(&two_by_two()).expect("layout");
        assert!(
            !layout.rx_ports.contains(&"voltage2".to_string()),
            "a channel with no gain is not a receive port"
        );
        assert!(!layout.tx_ports.contains(&"voltage3".to_string()));
    }

    #[test]
    fn a_one_by_one_radio_reports_one_lane_and_no_transmitter() {
        let layout = Layout::read(&one_by_one()).expect("layout");
        assert_eq!(layout.rx_streams(), 1);
        assert_eq!(layout.tx_streams(), 0);
        assert!(layout.tx.is_none());
    }

    #[test]
    fn a_buffer_reports_the_bytes_and_elements_the_asked_for_lanes_take() {
        let layout = Layout::read(&two_by_two()).expect("layout");
        let rx = layout.rx.as_ref().expect("rx buffer");
        assert_eq!(rx.device, "cf-ad9361-lpc");
        assert_eq!(rx.lanes(), 2);
        assert_eq!(rx.scan_total, 4);
        assert_eq!(rx.elements(1), vec![0, 1]);
        assert_eq!(rx.elements(2), vec![0, 1, 2, 3]);
        assert_eq!(rx.sample_bytes(1), 4);
        assert_eq!(rx.sample_bytes(2), 8);
        assert_eq!(rx.format.bits, 12);
        assert_eq!(
            layout.tx.as_ref().expect("tx buffer").format.bits,
            16,
            "the transmit path takes the full word"
        );
    }

    #[test]
    fn a_context_without_a_transceiver_is_refused_by_name() {
        let bare = context(
            r#"<context name="n" ><device id="iio:device0" name="xadc" >
<channel id="voltage0" type="input" ><attribute name="raw" value="1" /></channel>
</device></context>"#,
        );
        let error = Layout::read(&bare).expect_err("refused");
        assert!(error.to_string().contains("xadc"), "{error}");
        assert!(matches!(error, DeviceError::Unsupported(_)));
    }

    #[test]
    fn a_transceiver_with_no_receive_buffer_is_refused() {
        let no_buffer = context(
            r#"<context name="n" ><device id="iio:device0" name="ad9361-phy" >
<channel id="altvoltage0" name="RX_LO" type="output" >
<attribute name="frequency" value="1" /></channel>
<channel id="voltage0" type="input" ><attribute name="hardwaregain" value="1" /></channel>
</device></context>"#,
        );
        let error = Layout::read(&no_buffer).expect_err("refused");
        assert!(error.to_string().contains("no receive buffer"), "{error}");
    }

    #[test]
    fn an_available_list_is_named_after_the_attribute_it_bounds() {
        assert_eq!(available(HARDWAREGAIN), "hardwaregain_available");
        assert_eq!(available(FREQUENCY), "frequency_available");
    }
}

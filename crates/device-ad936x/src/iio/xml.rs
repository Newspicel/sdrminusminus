use sdrmm_device::DeviceError;

/// The IIO context as IIOD describes itself: which devices exist, which channels each carries,
/// and what a sample of a streaming channel looks like on the wire.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    pub(crate) description: String,
    pub(crate) attributes: Vec<(String, String)>,
    pub(crate) devices: Vec<Device>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Device {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) channels: Vec<Channel>,
    pub(crate) attributes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Channel {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) output: bool,
    pub(crate) scan: Option<Scan>,
    pub(crate) attributes: Vec<Attribute>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Attribute {
    pub(crate) name: String,
    pub(crate) value: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scan {
    pub(crate) index: u32,
    pub(crate) format: Format,
}

/// How one scan element sits in a buffer: `le:S12/16>>0` is a 12-bit signed value in a
/// little-endian 16-bit slot, which is what the AD936x receive path delivers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) little_endian: bool,
    pub(crate) signed: bool,
    pub(crate) bits: u32,
    pub(crate) storage_bits: u32,
    pub(crate) shift: u32,
}

impl Format {
    pub(crate) const fn storage_bytes(self) -> usize {
        (self.storage_bits as usize).div_ceil(8)
    }

    fn parse(text: &str) -> Option<Self> {
        let (endian, rest) = text.split_once(':')?;
        let sign = rest.chars().next()?;
        let (bits, rest) = rest[sign.len_utf8()..].split_once('/')?;
        let (storage_bits, shift) = match rest.split_once(">>") {
            Some((storage, shift)) => (storage, shift.trim()),
            None => (rest, "0"),
        };
        Some(Self {
            little_endian: !endian.eq_ignore_ascii_case("be"),
            signed: sign.eq_ignore_ascii_case(&'s'),
            bits: bits.trim().parse().ok()?,
            storage_bits: storage_bits.trim().parse().ok()?,
            shift: shift.parse().ok()?,
        })
    }
}

impl Context {
    pub(crate) fn parse(xml: &str) -> Result<Self, DeviceError> {
        let mut context = Self::default();
        let mut device: Option<Device> = None;
        let mut channel: Option<Channel> = None;
        for tag in Tags::new(xml) {
            match tag.name {
                "context" if tag.opening => {
                    context.description = tag.get("description").unwrap_or_default();
                }
                "context-attribute" => {
                    if let (Some(name), Some(value)) = (tag.get("name"), tag.get("value")) {
                        context.attributes.push((name, value));
                    }
                }
                "device" if tag.opening => {
                    device = Some(Device {
                        id: tag.get("id").unwrap_or_default(),
                        name: tag.get("name").unwrap_or_default(),
                        ..Device::default()
                    });
                }
                "device" => {
                    if let Some(mut done) = device.take() {
                        if let Some(open) = channel.take() {
                            done.channels.push(open);
                        }
                        context.devices.push(done);
                    }
                }
                "channel" if tag.opening => {
                    if let (Some(device), Some(open)) = (device.as_mut(), channel.take()) {
                        device.channels.push(open);
                    }
                    channel = Some(Channel {
                        id: tag.get("id").unwrap_or_default(),
                        name: tag.get("name"),
                        output: tag.get("type").as_deref() == Some("output"),
                        ..Channel::default()
                    });
                }
                "channel" => {
                    if let (Some(device), Some(done)) = (device.as_mut(), channel.take()) {
                        device.channels.push(done);
                    }
                }
                "scan-element" => {
                    if let Some(open) = channel.as_mut() {
                        open.scan = scan(&tag);
                    }
                }
                "attribute" => match (channel.as_mut(), device.as_mut(), tag.get("name")) {
                    (Some(open), _, Some(name)) => open.attributes.push(Attribute {
                        name,
                        value: tag.get("value"),
                    }),
                    (None, Some(device), Some(name)) => device.attributes.push(name),
                    _ => {}
                },
                _ => {}
            }
        }
        if context.devices.is_empty() {
            return Err(DeviceError::Io(
                "the radio described a context with no devices in it".to_string(),
            ));
        }
        Ok(context)
    }

    pub(crate) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn device(&self, name: &str) -> Option<&Device> {
        self.devices
            .iter()
            .find(|device| device.name == name || device.id == name)
    }
}

impl Device {
    pub(crate) fn channel(&self, id: &str, output: bool) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|channel| channel.id == id && channel.output == output)
    }

    /// The buffer channels of one direction, in scan order, which is the order their samples are
    /// interleaved in.
    pub(crate) fn scan_channels(&self, output: bool) -> Vec<&Channel> {
        let mut found: Vec<&Channel> = self
            .channels
            .iter()
            .filter(|channel| channel.output == output && channel.scan.is_some())
            .collect();
        found.sort_by_key(|channel| channel.scan.map_or(u32::MAX, |scan| scan.index));
        found
    }
}

impl Channel {
    pub(crate) fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|attr| attr.name == name)
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.attribute(name).is_some()
    }
}

fn scan(tag: &Tag<'_>) -> Option<Scan> {
    Some(Scan {
        index: tag.get("index")?.parse().ok()?,
        format: Format::parse(&tag.get("format")?)?,
    })
}

#[derive(Debug)]
struct Tag<'a> {
    name: &'a str,
    body: &'a str,
    opening: bool,
}

impl Tag<'_> {
    fn get(&self, key: &str) -> Option<String> {
        let mut rest = self.body;
        while let Some(at) = rest.find(key) {
            let after = &rest[at + key.len()..];
            let before_is_space = at == 0 || rest[..at].ends_with(|c: char| c.is_whitespace());
            match after.strip_prefix("=\"") {
                Some(value) if before_is_space => {
                    let end = value.find('"')?;
                    return Some(unescape(&value[..end]));
                }
                _ => rest = after,
            }
        }
        None
    }
}

/// Walks the tags of a document without building a tree, which is all a context description needs
/// and keeps a malformed one from costing anything.
struct Tags<'a> {
    rest: &'a str,
}

impl<'a> Tags<'a> {
    const fn new(xml: &'a str) -> Self {
        Self { rest: xml }
    }
}

impl<'a> Iterator for Tags<'a> {
    type Item = Tag<'a>;

    fn next(&mut self) -> Option<Tag<'a>> {
        loop {
            let open = self.rest.find('<')?;
            let after = &self.rest[open + 1..];
            let close = after.find('>')?;
            let inner = &after[..close];
            self.rest = &after[close + 1..];
            if inner.starts_with('?') || inner.starts_with('!') {
                continue;
            }
            let (closing, inner) = match inner.strip_prefix('/') {
                Some(inner) => (true, inner),
                None => (false, inner),
            };
            let inner = inner.strip_suffix('/').unwrap_or(inner);
            let name_end = inner
                .find(|c: char| c.is_whitespace())
                .unwrap_or(inner.len());
            let (name, body) = inner.split_at(name_end);
            if name.is_empty() {
                continue;
            }
            return Some(Tag {
                name,
                body,
                opening: !closing,
            });
        }
    }
}

fn unescape(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE context [<!ELEMENT context (device)*>]>
<context name="network" description="192.168.1.10 Linux (none) 4.19.0 armv7l" >
<context-attribute name="hw_model" value="Analog Devices ANTSDR Rev.C (Z7020-AD9361)" />
<context-attribute name="hw_serial" value="1044734c960500111e002e0041984fc267" />
<device id="iio:device0" name="ad9361-phy" >
<channel id="voltage0" type="input" >
<attribute name="hardwaregain" filename="in_voltage0_hardwaregain" value="71.000000 dB" />
<attribute name="hardwaregain_available" filename="in_voltage0_hardwaregain_available" value="[-3 1 71]" />
</channel>
<channel id="altvoltage0" name="RX_LO" type="output" >
<attribute name="frequency" filename="out_altvoltage0_RX_LO_frequency" value="2400000000" />
</channel>
<attribute name="ensm_mode" value="fdd" />
<debug-attribute name="loopback" value="0" />
</device>
<device id="iio:device3" name="cf-ad9361-lpc" >
<channel id="voltage1" type="input" >
<scan-element index="1" format="le:S12/16&gt;&gt;0" />
</channel>
<channel id="voltage0" type="input" >
<scan-element index="0" format="le:S12/16&gt;&gt;0" />
</channel>
<buffer-attribute name="watermark" value="2048" />
</device>
</context>"#;

    fn context() -> Context {
        Context::parse(SAMPLE).expect("a well-formed context")
    }

    #[test]
    fn the_context_names_itself_and_its_devices() {
        let context = context();
        assert!(context.description.contains("192.168.1.10"));
        assert_eq!(
            context.attribute("hw_model"),
            Some("Analog Devices ANTSDR Rev.C (Z7020-AD9361)")
        );
        assert_eq!(context.devices.len(), 2);
        assert_eq!(context.device("ad9361-phy").expect("phy").id, "iio:device0");
        assert!(context.device("iio:device3").is_some(), "found by id too");
        assert!(context.device("nothing-like-it").is_none());
    }

    #[test]
    fn channels_keep_their_direction_and_extended_name() {
        let phy = context().device("ad9361-phy").expect("phy").clone();
        let rx = phy.channel("voltage0", false).expect("rx port");
        assert!(rx.has("hardwaregain"));
        assert_eq!(
            rx.attribute("hardwaregain_available")
                .and_then(|attr| attr.value.clone()),
            Some("[-3 1 71]".to_string())
        );
        let lo = phy.channel("altvoltage0", true).expect("rx lo");
        assert_eq!(lo.name.as_deref(), Some("RX_LO"));
        assert!(phy.channel("altvoltage0", false).is_none());
        assert_eq!(phy.attributes, vec!["ensm_mode".to_string()]);
    }

    #[test]
    fn scan_channels_come_back_in_the_order_their_samples_arrive() {
        let rx = context().device("cf-ad9361-lpc").expect("rx").clone();
        let scan = rx.scan_channels(false);
        assert_eq!(
            scan.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["voltage0", "voltage1"],
            "the document lists them out of order"
        );
        assert!(rx.scan_channels(true).is_empty());
    }

    #[test]
    fn a_scan_format_survives_being_escaped_in_the_document() {
        let rx = context().device("cf-ad9361-lpc").expect("rx").clone();
        let format = rx.scan_channels(false)[0].scan.expect("scan").format;
        assert_eq!(
            format,
            Format {
                little_endian: true,
                signed: true,
                bits: 12,
                storage_bits: 16,
                shift: 0,
            }
        );
        assert_eq!(format.storage_bytes(), 2);
    }

    #[test]
    fn every_format_shape_the_kernel_writes_is_understood() {
        let be = Format::parse("be:u8/8>>0").expect("big endian unsigned");
        assert!(!be.little_endian);
        assert!(!be.signed);
        assert_eq!(be.storage_bytes(), 1);

        let shifted = Format::parse("le:S14/16>>2").expect("shifted");
        assert_eq!(shifted.shift, 2);
        assert_eq!(Format::parse("le:S16/16").expect("no shift").shift, 0);
        assert_eq!(
            Format::parse("le:S32/32>>0").expect("wide").storage_bytes(),
            4
        );

        assert!(Format::parse("nonsense").is_none());
        assert!(Format::parse("le:S12").is_none());
    }

    #[test]
    fn a_malformed_format_from_the_radio_is_refused_rather_than_panicked_on() {
        assert!(Format::parse("le:").is_none());
        assert!(Format::parse("le:S").is_none());
        assert!(Format::parse(":").is_none());
        let odd = Format::parse("le:é12/16>>0").expect("an unknown sign is unsigned");
        assert!(!odd.signed);
        assert_eq!(odd.bits, 12);
    }

    #[test]
    fn an_attribute_name_is_matched_whole_and_not_as_a_prefix() {
        let phy = context().device("ad9361-phy").expect("phy").clone();
        let rx = phy.channel("voltage0", false).expect("rx port");
        assert_eq!(rx.attributes.len(), 2);
        assert!(rx.has("hardwaregain_available"));
        assert_eq!(
            rx.attribute("hardwaregain").and_then(|a| a.value.clone()),
            Some("71.000000 dB".to_string()),
            "the shorter name must not pick up the longer one's value"
        );
    }

    #[test]
    fn a_document_with_no_devices_is_refused_rather_than_half_believed() {
        let error = Context::parse("<context name=\"x\" ></context>").expect_err("refused");
        assert!(error.to_string().contains("no devices"), "{error}");
        assert!(Context::parse("").is_err());
    }

    #[test]
    fn escapes_come_back_as_the_characters_they_stand_for() {
        assert_eq!(unescape("a &amp; b &lt;c&gt;"), "a & b <c>");
        assert_eq!(unescape("&quot;q&apos;"), "\"q'");
        assert_eq!(unescape("plain"), "plain");
    }
}

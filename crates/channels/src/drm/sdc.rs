use super::{
    aac::{AudioConfig, AudioMode, Coding, MAX_CONFIG},
    bits::{BitReader, BitWriter, byte_bits, crc},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stream {
    pub higher: u16,
    pub lower: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Multiplex {
    pub protection_higher: u8,
    pub protection_lower: u8,
    pub streams: Vec<Stream>,
}

impl Multiplex {
    #[must_use]
    pub fn higher_bytes(&self) -> usize {
        self.streams
            .iter()
            .map(|stream| usize::from(stream.higher))
            .sum()
    }

    #[must_use]
    pub fn lower_bytes(&self) -> usize {
        self.streams
            .iter()
            .map(|stream| usize::from(stream.lower))
            .sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audio {
    pub stream: u8,
    pub config: AudioConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Application {
    pub stream: u8,
    pub packet_mode: bool,
    pub domain: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sdc {
    pub afs: u8,
    pub multiplex: Option<Multiplex>,
    pub labels: [Option<String>; 4],
    pub audio: [Option<Audio>; 4],
    pub languages: [Option<(String, String)>; 4],
    pub applications: [Option<Application>; 4],
    pub time: Option<(u32, u8, u8)>,
}

const AAC_RATES: [u32; 8] = [0, 12_000, 0, 24_000, 0, 48_000, 0, 0];
const XHE_RATES: [u32; 8] = [
    9_600, 12_000, 16_000, 19_200, 24_000, 32_000, 38_400, 48_000,
];

fn multiplex(body: &mut BitReader<'_>, length: usize) -> Option<Multiplex> {
    let protection_higher = body.read(2)? as u8;
    let protection_lower = body.read(2)? as u8;
    let streams = (0..length / 3)
        .map(|_| {
            Some(Stream {
                higher: body.read(12)? as u16,
                lower: body.read(12)? as u16,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Multiplex {
        protection_higher,
        protection_lower,
        streams,
    })
}

fn label(body: &mut BitReader<'_>, length: usize) -> Option<(u8, String)> {
    let short_id = body.read(2)? as u8;
    body.skip(2)?;
    let bytes = (0..length)
        .map(|_| body.read(8).map(|byte| byte as u8))
        .collect::<Option<Vec<u8>>>()?;
    let text = bytes.strip_prefix(&[0x01]).unwrap_or(&bytes);
    let text = String::from_utf8_lossy(text)
        .trim_matches(|c: char| c.is_control() || c.is_whitespace())
        .to_owned();
    Some((short_id, text))
}

fn audio(body: &mut BitReader<'_>, length: usize) -> Option<(u8, Audio)> {
    let short_id = body.read(2)? as u8;
    let stream = body.read(2)? as u8;
    let coding = match body.read(2)? {
        0 => Coding::Aac,
        3 => Coding::Xhe,
        _ => return None,
    };
    let sbr = body.read(1)? == 1 && coding == Coding::Aac;
    let mode = match body.read(2)? {
        0 => AudioMode::Mono,
        1 if coding == Coding::Aac => AudioMode::ParametricStereo,
        2 => AudioMode::Stereo,
        _ => return None,
    };
    let rate_code = body.read(3)? as u8;
    let rate_hz = match coding {
        Coding::Aac => AAC_RATES[usize::from(rate_code)],
        Coding::Xhe => XHE_RATES[usize::from(rate_code)],
    };
    if rate_hz == 0 {
        return None;
    }
    let text = body.read(1)? == 1;
    body.skip(1)?;
    let surround = (body.read(5)? >> 2) as u8;
    body.skip(1)?;
    let extra = length.checked_sub(2)?.min(MAX_CONFIG);
    let mut config = [0u8; MAX_CONFIG];
    for byte in config.iter_mut().take(extra) {
        *byte = body.read(8)? as u8;
    }
    Some((
        short_id,
        Audio {
            stream,
            config: AudioConfig {
                coding,
                sbr,
                mode,
                rate_hz,
                rate_code,
                text,
                surround,
                config,
                config_length: if coding == Coding::Xhe {
                    extra as u8
                } else {
                    0
                },
            },
        },
    ))
}

fn application(body: &mut BitReader<'_>) -> Option<(u8, Application)> {
    let short_id = body.read(2)? as u8;
    let stream = body.read(2)? as u8;
    let packet_mode = body.read(1)? == 1;
    let domain = if packet_mode {
        body.skip(4)?;
        let domain = body.read(3)? as u8;
        body.skip(8)?;
        domain
    } else {
        body.skip(4)?;
        body.read(3)? as u8
    };
    Some((
        short_id,
        Application {
            stream,
            packet_mode,
            domain,
        },
    ))
}

fn language(body: &mut BitReader<'_>) -> Option<(u8, (String, String))> {
    let short_id = body.read(2)? as u8;
    body.skip(2)?;
    let text = |count: usize, body: &mut BitReader<'_>| -> Option<String> {
        let bytes = (0..count)
            .map(|_| body.read(8).map(|byte| byte as u8))
            .collect::<Option<Vec<u8>>>()?;
        Some(
            bytes
                .iter()
                .map(|&byte| char::from(byte))
                .filter(|&c| c != '-')
                .collect(),
        )
    };
    let language = text(3, body)?;
    let country = text(2, body)?;
    Some((short_id, (language, country)))
}

fn time(body: &mut BitReader<'_>) -> Option<(u32, u8, u8)> {
    let mjd = body.read(17)?;
    let hours = body.read(5)? as u8;
    let minutes = body.read(6)? as u8;
    Some((mjd, hours, minutes))
}

impl Sdc {
    #[must_use]
    pub fn parse(bits: &[bool], data_bytes: usize) -> Option<Self> {
        let total = 4 + 8 * data_bytes + 16;
        if bits.len() < total {
            return None;
        }
        let mut bytes = Vec::with_capacity(data_bytes + 3);
        super::bits::pack(&bits[..total], &mut bytes);
        let mut check = vec![0u8];
        check
            .extend((0..data_bytes + 2).map(|index| (bytes[index] << 4) | (bytes[index + 1] >> 4)));
        check[0] = bytes[0] >> 4;
        let stored = u32::from(check[data_bytes + 1]) << 8 | u32::from(check[data_bytes + 2]);
        if crc(0x1021, 16, byte_bits(&check[..data_bytes + 1])) != stored {
            return None;
        }
        let mut sdc = Self {
            afs: check[0],
            ..Self::default()
        };
        let mut reader = BitReader::new(&check[1..data_bytes + 1]);
        while reader.remaining() >= 16 {
            let length = reader.read(7)? as usize;
            let version = reader.read(1)?;
            let kind = reader.read(4)?;
            if length == 0 && version == 0 && kind == 0 {
                break;
            }
            let start = reader.position();
            let end = start + 4 + 8 * length;
            if end > start + reader.remaining() {
                break;
            }
            sdc.absorb(&mut reader, kind, version == 1, length);
            reader.seek(end);
        }
        Some(sdc)
    }

    fn absorb(&mut self, body: &mut BitReader<'_>, kind: u32, next: bool, length: usize) {
        match kind {
            0 if !next => self.multiplex = multiplex(body, length),
            1 => {
                if let Some((id, text)) = label(body, length) {
                    self.labels[usize::from(id)] = Some(text);
                }
            }
            5 if !next => {
                if let Some((id, application)) = application(body) {
                    self.applications[usize::from(id)] = Some(application);
                }
            }
            8 => self.time = time(body),
            9 if !next => {
                if let Some((id, entry)) = audio(body, length) {
                    self.audio[usize::from(id)] = Some(entry);
                }
            }
            12 => {
                if let Some((id, entry)) = language(body) {
                    self.languages[usize::from(id)] = Some(entry);
                }
            }
            _ => {}
        }
    }

    pub fn merge(&mut self, update: Self) {
        self.afs = update.afs;
        if update.multiplex.is_some() {
            self.multiplex = update.multiplex;
        }
        if update.time.is_some() {
            self.time = update.time;
        }
        for index in 0..4 {
            if update.labels[index].is_some() {
                self.labels[index].clone_from(&update.labels[index]);
            }
            if update.audio[index].is_some() {
                self.audio[index] = update.audio[index];
            }
            if update.languages[index].is_some() {
                self.languages[index].clone_from(&update.languages[index]);
            }
            if update.applications[index].is_some() {
                self.applications[index] = update.applications[index];
            }
        }
    }
}

fn entity(writer: &mut BitWriter, kind: u32, body: &BitWriter) {
    let length = (body.bit_len() - 4).div_ceil(8);
    writer.put(length as u32, 7);
    writer.put(0, 1);
    writer.put(kind, 4);
    let reader = BitReader::new(body.bytes());
    writer.copy(&reader, 0, body.bit_len());
    for _ in body.bit_len()..4 + 8 * length {
        writer.bit(false);
    }
}

#[must_use]
pub fn encode(sdc: &Sdc, data_bytes: usize, total_bits: usize) -> Option<Vec<bool>> {
    let mut data = BitWriter::with_capacity(data_bytes);
    if let Some(multiplex) = &sdc.multiplex {
        let mut body = BitWriter::default();
        body.put(u32::from(multiplex.protection_higher), 2);
        body.put(u32::from(multiplex.protection_lower), 2);
        for stream in &multiplex.streams {
            body.put(u32::from(stream.higher), 12);
            body.put(u32::from(stream.lower), 12);
        }
        entity(&mut data, 0, &body);
    }
    for (id, label) in sdc.labels.iter().enumerate() {
        if let Some(label) = label {
            let mut body = BitWriter::default();
            body.put(id as u32, 2);
            body.put(0, 2);
            for &byte in label.as_bytes() {
                body.put(u32::from(byte), 8);
            }
            entity(&mut data, 1, &body);
        }
    }
    for (id, audio) in sdc.audio.iter().enumerate() {
        if let Some(audio) = audio {
            let config = audio.config;
            let mut body = BitWriter::default();
            body.put(id as u32, 2);
            body.put(u32::from(audio.stream), 2);
            body.put(if config.coding == Coding::Xhe { 3 } else { 0 }, 2);
            body.put(u32::from(config.sbr), 1);
            body.put(
                match config.mode {
                    AudioMode::Mono => 0,
                    AudioMode::ParametricStereo => 1,
                    AudioMode::Stereo => 2,
                },
                2,
            );
            body.put(u32::from(config.rate_code), 3);
            body.put(u32::from(config.text), 1);
            body.put(0, 1);
            body.put(u32::from(config.surround) << 2, 5);
            body.put(0, 1);
            for &byte in config.codec_config() {
                body.put(u32::from(byte), 8);
            }
            entity(&mut data, 9, &body);
        }
    }
    for (id, entry) in sdc.languages.iter().enumerate() {
        if let Some((language, country)) = entry {
            let mut body = BitWriter::default();
            body.put(id as u32, 2);
            body.put(0, 2);
            for byte in language.bytes().chain(country.bytes()) {
                body.put(u32::from(byte), 8);
            }
            entity(&mut data, 12, &body);
        }
    }
    for (id, entry) in sdc.applications.iter().enumerate() {
        if let Some(application) = entry {
            let mut body = BitWriter::default();
            body.put(id as u32, 2);
            body.put(u32::from(application.stream), 2);
            body.put(u32::from(application.packet_mode), 1);
            body.put(0, 4);
            body.put(u32::from(application.domain), 3);
            if application.packet_mode {
                body.put(0, 8);
            }
            entity(&mut data, 5, &body);
        }
    }
    if data.bit_len() > 8 * data_bytes {
        return None;
    }
    while data.bit_len() < 8 * data_bytes {
        data.bit(false);
    }
    let mut check = vec![sdc.afs & 0x0F];
    check.extend_from_slice(data.bytes());
    let value = crc(0x1021, 16, byte_bits(&check));
    let mut bits = Vec::with_capacity(total_bits);
    bits.extend((0..4).rev().map(|shift| sdc.afs >> shift & 1 == 1));
    bits.extend(byte_bits(data.bytes()));
    bits.extend((0..16).rev().map(|shift| value >> shift & 1 == 1));
    bits.resize(total_bits, false);
    Some(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdc_blocks_round_trip() {
        let mut sdc = Sdc {
            afs: 3,
            multiplex: Some(Multiplex {
                protection_higher: 0,
                protection_lower: 1,
                streams: vec![Stream {
                    higher: 0,
                    lower: 600,
                }],
            }),
            ..Sdc::default()
        };
        sdc.labels[1] = Some("Rust Wave".to_owned());
        sdc.languages[1] = Some(("eng".to_owned(), "de".to_owned()));
        sdc.applications[2] = Some(Application {
            stream: 1,
            packet_mode: true,
            domain: 0,
        });
        let mut config = [0; MAX_CONFIG];
        config[0] = 0x55;
        sdc.audio[1] = Some(Audio {
            stream: 0,
            config: AudioConfig {
                coding: Coding::Xhe,
                sbr: false,
                mode: AudioMode::Stereo,
                rate_hz: 24_000,
                rate_code: 4,
                text: true,
                surround: 0,
                config,
                config_length: 1,
            },
        });
        let bits = encode(&sdc, 47, 399).expect("fits");
        assert_eq!(Sdc::parse(&bits, 47), Some(sdc));
        let mut damaged = bits.clone();
        damaged[100] = !damaged[100];
        assert_eq!(Sdc::parse(&damaged, 47), None);
    }
}

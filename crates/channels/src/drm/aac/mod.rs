mod hcr;
mod huffman;
mod syntax;
mod tables;
mod usac;

#[cfg(any(test, feature = "synth"))]
mod encode;
#[cfg(any(test, feature = "synth"))]
pub use encode::drm_frame_padded;

use self::{
    huffman::spectral,
    syntax::{
        Codeword, IcsInfo, Section, band_offsets, read_sections, skip_scalefactors, skip_tns,
    },
};
use super::bits::{BitReader, BitWriter, crc};

pub const MAX_CONFIG: usize = 64;
const SBR_EXTENSION: u32 = 13;
const MAX_FILL_BYTES: usize = 269;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    Aac,
    Xhe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMode {
    Mono,
    ParametricStereo,
    Stereo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub coding: Coding,
    pub sbr: bool,
    pub mode: AudioMode,
    pub rate_hz: u32,
    pub rate_code: u8,
    pub text: bool,
    pub surround: u8,
    pub config: [u8; MAX_CONFIG],
    pub config_length: u8,
}

impl AudioConfig {
    #[must_use]
    pub fn codec_config(&self) -> &[u8] {
        &self.config[..usize::from(self.config_length)]
    }

    #[must_use]
    pub const fn stereo(&self) -> bool {
        matches!(self.mode, AudioMode::Stereo)
    }

    #[must_use]
    pub const fn output_rate_hz(&self) -> u32 {
        if self.sbr {
            self.rate_hz * 2
        } else {
            self.rate_hz
        }
    }

    #[must_use]
    pub fn describe(&self) -> String {
        let codec = match (self.coding, self.sbr, self.mode) {
            (Coding::Xhe, ..) => "xHE-AAC",
            (Coding::Aac, true, AudioMode::ParametricStereo) => "HE-AAC v2",
            (Coding::Aac, true, _) => "HE-AAC",
            (Coding::Aac, false, _) => "AAC",
        };
        let channels = match self.mode {
            AudioMode::Mono => "mono",
            AudioMode::ParametricStereo => "PS",
            AudioMode::Stereo => "stereo",
        };
        format!("{codec} {} kHz {channels}", self.rate_hz / 1000)
    }
}

pub fn latm(frame: &[u8], config: &AudioConfig) -> Result<Vec<u8>, String> {
    match config.coding {
        Coding::Aac => {
            let block = standard(frame, config).map_err(str::to_owned)?;
            let asc = aac_config(config).ok_or("Unsupported DRM AAC sampling rate")?;
            Ok(crate::broadcast_media::latm::mux(
                &block,
                asc.bytes(),
                asc.bit_len(),
            ))
        }
        Coding::Xhe => {
            let asc = usac::audio_specific_config(config)
                .ok_or("Invalid xHE-AAC static configuration")?;
            Ok(crate::broadcast_media::latm::mux(
                frame,
                asc.bytes(),
                asc.bit_len(),
            ))
        }
    }
}

fn sample_index(rate_hz: u32) -> Option<u32> {
    Some(match rate_hz {
        48_000 => 3,
        24_000 => 6,
        16_000 => 8,
        12_000 => 9,
        _ => return None,
    })
}

fn aac_config(config: &AudioConfig) -> Option<BitWriter> {
    let mut writer = BitWriter::with_capacity(4);
    let object = match (config.sbr, config.mode) {
        (true, AudioMode::ParametricStereo) => 29,
        (true, _) => 5,
        (false, _) => 2,
    };
    writer.put(object, 5);
    writer.put(sample_index(config.rate_hz)?, 4);
    writer.put(if config.stereo() { 2 } else { 1 }, 4);
    if config.sbr {
        writer.put(sample_index(config.rate_hz * 2)?, 4);
        writer.put(2, 5);
    }
    writer.put(0b100, 3);
    Some(writer)
}

struct Channel {
    tns: Option<std::ops::Range<usize>>,
    gain: u8,
    sections: Vec<Section>,
    scalefactors: std::ops::Range<usize>,
    reordered: usize,
    longest: usize,
    words: Vec<Codeword>,
}

fn read_side(reader: &mut BitReader<'_>, info: IcsInfo) -> Option<(bool, Channel)> {
    let tns = reader.flag()?;
    if reader.flag()? {
        return None;
    }
    let gain = reader.read(8)? as u8;
    let sections = read_sections(reader, info, true)?;
    let scalefactors = skip_scalefactors(reader, info, &sections)?;
    let reordered = reader.read(14)? as usize;
    let longest = reader.read(6)? as usize;
    Some((
        tns,
        Channel {
            tns: None,
            gain,
            sections,
            scalefactors,
            reordered,
            longest,
            words: Vec::new(),
        },
    ))
}

struct Parsed {
    info: IcsInfo,
    ms: (u8, Vec<bool>),
    channels: Vec<Channel>,
    sbr: Vec<bool>,
}

fn parse(body: &[u8], stored_crc: u8, config: &AudioConfig) -> Result<Parsed, &'static str> {
    let damaged = "Damaged DRM AAC frame";
    let mut reader = BitReader::new(body);
    if reader.flag().ok_or(damaged)? {
        return Err(damaged);
    }
    let info = IcsInfo::read_header(&mut reader).ok_or(damaged)?;
    let offsets = band_offsets(config.rate_hz, info.short()).ok_or("Unsupported AAC rate")?;
    if usize::from(info.max_sfb) + 1 > offsets.len() {
        return Err(damaged);
    }
    let ms = if config.stereo() {
        let present = reader.read(2).ok_or(damaged)? as u8;
        let used = if present == 1 {
            (0..info.groups().len() * usize::from(info.max_sfb))
                .map(|_| reader.flag())
                .collect::<Option<Vec<bool>>>()
                .ok_or(damaged)?
        } else {
            Vec::new()
        };
        if present == 3 {
            return Err(damaged);
        }
        (present, used)
    } else {
        (0, Vec::new())
    };
    let mut channels = Vec::with_capacity(2);
    let mut tns = Vec::with_capacity(2);
    for _ in 0..if config.stereo() { 2 } else { 1 } {
        let (present, channel) = read_side(&mut reader, info).ok_or(damaged)?;
        tns.push(present);
        channels.push(channel);
    }
    for (channel, present) in channels.iter_mut().zip(tns) {
        if present {
            channel.tns = Some(skip_tns(&mut reader, info).ok_or(damaged)?);
        }
    }
    let protected = reader.position();
    if crc(0x1D, 8, (0..protected).map(|at| reader.bit_at(at))) != u32::from(stored_crc) {
        return Err("DRM AAC frame CRC failure");
    }
    for channel in &mut channels {
        let slots = hcr::order(info, &channel.sections, offsets);
        let start = reader.position();
        channel.words = hcr::decode(&reader, start, channel.reordered, channel.longest, &slots)
            .ok_or("DRM AAC spectral data damaged")?;
        reader.skip(channel.reordered).ok_or(damaged)?;
    }
    let sbr = if config.sbr && reader.remaining() > 8 {
        let end = reader.position() + reader.remaining();
        (reader.position()..end - 8)
            .rev()
            .map(|at| reader.bit_at(at))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Parsed {
        info,
        ms,
        channels,
        sbr,
    })
}

fn write_info(writer: &mut BitWriter, info: IcsInfo) {
    writer.bit(false);
    info.write_header(writer);
    if !info.short() {
        writer.bit(false);
    }
}

fn write_channel(
    writer: &mut BitWriter,
    reader: &BitReader<'_>,
    channel: &Channel,
    info: IcsInfo,
    common: bool,
    offsets: &[u16],
) -> Result<(), &'static str> {
    writer.put(u32::from(channel.gain), 8);
    if !common {
        write_info(writer, info);
    }
    syntax::write_sections(writer, info, &channel.sections, false);
    writer.copy(reader, channel.scalefactors.start, channel.scalefactors.end);
    writer.bit(false);
    writer.bit(channel.tns.is_some());
    if let Some(tns) = &channel.tns {
        writer.copy(reader, tns.start, tns.end);
    }
    writer.bit(false);
    let mut words = channel.words.clone();
    words.sort_by_key(|word| (word.window, word.line));
    for (codebook, window, line) in syntax::standard_positions(info, &channel.sections, offsets) {
        let word = words
            .binary_search_by_key(&(window, line), |word| (word.window, word.line))
            .map_err(|_| "DRM AAC codeword missing")?;
        if !spectral(codebook) {
            return Err("DRM AAC codeword missing");
        }
        words[word].write(writer);
    }
    Ok(())
}

fn write_sbr(writer: &mut BitWriter, sbr: &[bool]) {
    if sbr.is_empty() {
        return;
    }
    let bits = sbr.len().min(MAX_FILL_BYTES * 8 - 4);
    let bytes = (bits + 4).div_ceil(8);
    writer.put(6, 3);
    if bytes < 15 {
        writer.put(bytes as u32, 4);
    } else {
        writer.put(15, 4);
        writer.put((bytes - 14) as u32, 8);
    }
    writer.put(SBR_EXTENSION, 4);
    for &bit in &sbr[..bits] {
        writer.bit(bit);
    }
    for _ in bits + 4..bytes * 8 {
        writer.bit(false);
    }
}

fn standard(frame: &[u8], config: &AudioConfig) -> Result<Vec<u8>, &'static str> {
    let (&stored_crc, body) = frame.split_first().ok_or("Empty DRM AAC frame")?;
    let parsed = parse(body, stored_crc, config)?;
    let reader = BitReader::new(body);
    let offsets =
        band_offsets(config.rate_hz, parsed.info.short()).ok_or("Unsupported AAC rate")?;
    let mut writer = BitWriter::with_capacity(body.len() + 32);
    if config.stereo() {
        writer.put(1, 3);
        writer.put(0, 4);
        writer.bit(true);
        write_info(&mut writer, parsed.info);
        writer.put(u32::from(parsed.ms.0), 2);
        for &used in &parsed.ms.1 {
            writer.bit(used);
        }
    } else {
        writer.put(0, 3);
        writer.put(0, 4);
    }
    for channel in &parsed.channels {
        write_channel(
            &mut writer,
            &reader,
            channel,
            parsed.info,
            config.stereo(),
            offsets,
        )?;
    }
    write_sbr(&mut writer, &parsed.sbr);
    writer.put(7, 3);
    writer.align();
    Ok(writer.into_bytes())
}

#[cfg(test)]
mod tests;

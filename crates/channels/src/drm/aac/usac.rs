use super::{AudioConfig, AudioMode};
use crate::drm::bits::{BitReader, BitWriter};

const RATES: [u32; 8] = [
    9_600, 12_000, 16_000, 19_200, 24_000, 32_000, 38_400, 48_000,
];
const USAC_INDEX: [u32; 8] = [0x1b, 0x09, 0x08, 0x17, 0x06, 0x05, 0x12, 0x03];
const AOT_ESCAPE: u32 = 31;
const AOT_USAC: u32 = 42;
const ELEMENT_SCE: u32 = 0;
const ELEMENT_CPE: u32 = 1;
const ELEMENT_EXTENSION: u32 = 3;

fn mpeg_index(rate: u32) -> Option<u32> {
    Some(match rate {
        48_000 => 3,
        32_000 => 5,
        24_000 => 6,
        16_000 => 8,
        12_000 => 9,
        _ => return None,
    })
}

fn read_escaped(reader: &mut BitReader<'_>, widths: [u32; 3]) -> Option<u32> {
    let mut value = reader.read(widths[0])?;
    if value == (1 << widths[0]) - 1 {
        let next = reader.read(widths[1])?;
        value += next;
        if next == (1 << widths[1]) - 1 && widths[2] > 0 {
            value += reader.read(widths[2])?;
        }
    }
    Some(value)
}

fn write_escaped(writer: &mut BitWriter, value: u32, widths: [u32; 3]) {
    let first = (1 << widths[0]) - 1;
    if value < first {
        writer.put(value, widths[0]);
        return;
    }
    writer.put(first, widths[0]);
    let rest = value - first;
    let second = (1 << widths[1]) - 1;
    if rest < second || widths[2] == 0 {
        writer.put(rest.min(second), widths[1]);
        return;
    }
    writer.put(second, widths[1]);
    writer.put(rest - second, widths[2]);
}

fn copy(reader: &mut BitReader<'_>, writer: &mut BitWriter, width: u32) -> Option<u32> {
    let value = reader.read(width)?;
    writer.put(value, width);
    Some(value)
}

fn copy_sbr(reader: &mut BitReader<'_>, writer: &mut BitWriter) -> Option<()> {
    copy(reader, writer, 3)?;
    copy(reader, writer, 8)?;
    let extra_one = copy(reader, writer, 1)?;
    let extra_two = copy(reader, writer, 1)?;
    if extra_one == 1 {
        copy(reader, writer, 5)?;
    }
    if extra_two == 1 {
        copy(reader, writer, 6)?;
    }
    Some(())
}

fn copy_mps(reader: &mut BitReader<'_>, writer: &mut BitWriter, stereo_index: u32) -> Option<()> {
    copy(reader, writer, 6)?;
    let shaping = reader.read(1)?;
    writer.put(if shaping == 1 { 3 } else { 0 }, 2);
    writer.put(0, 2);
    copy(reader, writer, 2)?;
    if copy(reader, writer, 1)? == 1 {
        copy(reader, writer, 5)?;
    }
    if stereo_index > 1 {
        copy(reader, writer, 6)?;
    }
    Some(())
}

fn copy_extension(reader: &mut BitReader<'_>, writer: &mut BitWriter) -> Option<()> {
    let kind = read_escaped(reader, [4, 8, 16])?;
    write_escaped(writer, kind, [4, 8, 16]);
    let length = read_escaped(reader, [4, 8, 16])?;
    write_escaped(writer, length, [4, 8, 16]);
    if copy(reader, writer, 1)? == 1 {
        let default = read_escaped(reader, [8, 16, 0])?;
        write_escaped(writer, default, [8, 16, 0]);
    }
    copy(reader, writer, 1)?;
    for _ in 0..length {
        copy(reader, writer, 8)?;
    }
    Some(())
}

fn copy_config_extension(reader: &mut BitReader<'_>, writer: &mut BitWriter) -> Option<()> {
    let present = reader.read(1).unwrap_or(0);
    writer.put(present, 1);
    if present == 0 {
        return Some(());
    }
    let count = read_escaped(reader, [2, 4, 8])?;
    write_escaped(writer, count, [2, 4, 8]);
    for _ in 0..=count {
        let kind = read_escaped(reader, [4, 8, 16])?;
        write_escaped(writer, kind, [4, 8, 16]);
        let length = read_escaped(reader, [4, 8, 16])?;
        write_escaped(writer, length, [4, 8, 16]);
        for _ in 0..length {
            copy(reader, writer, 8)?;
        }
    }
    Some(())
}

fn element(reader: &mut BitReader<'_>, stereo: bool, sbr_ratio: u32) -> Option<BitWriter> {
    let mut writer = BitWriter::default();
    writer.put(0, 1);
    copy(reader, &mut writer, 1)?;
    if sbr_ratio > 0 {
        copy_sbr(reader, &mut writer)?;
        if stereo {
            let stereo_index = copy(reader, &mut writer, 2)?;
            if stereo_index > 0 {
                copy_mps(reader, &mut writer, stereo_index)?;
            }
        }
    }
    Some(writer)
}

#[must_use]
pub fn audio_specific_config(config: &AudioConfig) -> Option<BitWriter> {
    let code = usize::from(config.rate_code);
    let rate = *RATES.get(code)?;
    let stereo = match config.mode {
        AudioMode::Mono => false,
        AudioMode::Stereo => true,
        AudioMode::ParametricStereo => return None,
    };
    let channels = if stereo { 2 } else { 1 };
    let mut reader = BitReader::new(config.codec_config());
    let mut writer = BitWriter::with_capacity(32);
    writer.put(AOT_ESCAPE, 5);
    writer.put(AOT_USAC - 32, 6);
    match mpeg_index(rate) {
        Some(index) => writer.put(index, 4),
        None => {
            writer.put(15, 4);
            writer.put(rate, 24);
        }
    }
    writer.put(channels, 4);
    writer.put(USAC_INDEX[code], 5);
    let frame_length = reader.read(2)? + 1;
    writer.put(frame_length, 3);
    writer.put(channels, 5);
    let sbr_ratio = match frame_length {
        2 => 2,
        3 => 3,
        4 => 1,
        _ => 0,
    };
    let core = element(&mut reader, stereo, sbr_ratio)?;
    let extensions = read_escaped(&mut reader, [2, 4, 8])?;
    write_escaped(&mut writer, extensions, [4, 8, 16]);
    writer.put(if stereo { ELEMENT_CPE } else { ELEMENT_SCE }, 2);
    let core_reader = BitReader::new(core.bytes());
    writer.copy(&core_reader, 0, core.bit_len());
    for _ in 0..extensions {
        writer.put(ELEMENT_EXTENSION, 2);
        copy_extension(&mut reader, &mut writer)?;
    }
    copy_config_extension(&mut reader, &mut writer)?;
    Some(writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drm::aac::{Coding, MAX_CONFIG};

    fn config(mode: AudioMode, rate_code: u8, bytes: &[u8]) -> AudioConfig {
        let mut config = [0; MAX_CONFIG];
        config[..bytes.len()].copy_from_slice(bytes);
        AudioConfig {
            coding: Coding::Xhe,
            sbr: false,
            mode,
            rate_hz: RATES[usize::from(rate_code)],
            rate_code,
            text: false,
            surround: 0,
            config,
            config_length: bytes.len() as u8,
        }
    }

    #[test]
    fn a_mono_static_config_becomes_a_usac_config() {
        let asc = audio_specific_config(&config(AudioMode::Mono, 4, &[0b0010_0000]))
            .expect("a USAC configuration");
        let mut reader = BitReader::new(asc.bytes());
        assert_eq!(reader.read(5), Some(31));
        assert_eq!(reader.read(6), Some(10));
        assert_eq!(reader.read(4), Some(6));
        assert_eq!(reader.read(4), Some(1));
        assert_eq!(reader.read(5), Some(6));
        assert_eq!(reader.read(3), Some(1));
        assert_eq!(reader.read(5), Some(1));
        assert_eq!(reader.read(4), Some(0));
        assert_eq!(reader.read(2), Some(ELEMENT_SCE));
        assert_eq!(reader.read(2), Some(0b01));
    }

    #[test]
    fn odd_rates_are_spelled_out() {
        let asc =
            audio_specific_config(&config(AudioMode::Mono, 6, &[0])).expect("a USAC configuration");
        let mut reader = BitReader::new(asc.bytes());
        reader.skip(11);
        assert_eq!(reader.read(4), Some(15));
        assert_eq!(reader.read(24), Some(38_400));
    }
}

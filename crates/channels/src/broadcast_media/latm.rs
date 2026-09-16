use crate::dab::superframe::AudioFormat;

struct Bits {
    bytes: Vec<u8>,
    used: usize,
}

impl Bits {
    fn push(&mut self, value: usize, width: usize) {
        for bit in (0..width).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            self.bytes[self.used / 8] |= (((value >> bit) & 1) as u8) << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

pub fn wrap(unit: &[u8], format: AudioFormat) -> Result<Vec<u8>, &'static str> {
    if unit.len() > 8000 || format.surround != 0 || !matches!(format.sample_rate_hz, 32000 | 48000)
    {
        return Err("Unsupported DAB+ access-unit size or surround configuration");
    }
    let mut unit = unit;
    while unit.first().is_some_and(|byte| byte >> 5 == 4) {
        let count = usize::from(*unit.get(1).ok_or("Truncated DAB+ data element")?);
        let offset = if count == 255 {
            3 + count + usize::from(*unit.get(2).ok_or("Truncated DAB+ data element")?)
        } else {
            2 + count
        };
        unit = unit.get(offset..).ok_or("Truncated DAB+ data element")?;
    }
    let mut bits = Bits {
        bytes: Vec::with_capacity(unit.len() + 16),
        used: 0,
    };
    bits.push(0, 1);
    bits.push(0, 1);
    bits.push(1, 1);
    bits.push(0, 6);
    bits.push(0, 4);
    bits.push(0, 3);
    let extension = format.spectral_band_replication;
    bits.push(
        if extension {
            if format.parametric_stereo { 29 } else { 5 }
        } else {
            2
        },
        5,
    );
    let index = |rate| match rate {
        16000 => 8,
        24000 => 6,
        32000 => 5,
        48000 => 3,
        _ => 15,
    };
    bits.push(index(format.core_rate_hz()), 4);
    bits.push(if format.stereo_core { 2 } else { 1 }, 4);
    if extension {
        bits.push(index(format.output_rate_hz()), 4);
        bits.push(2, 5);
    }
    bits.push(1, 1);
    bits.push(0, 2);
    bits.push(0, 3);
    bits.push(255, 8);
    bits.push(0, 2);
    let mut length = unit.len();
    while length >= 255 {
        bits.push(255, 8);
        length -= 255;
    }
    bits.push(length, 8);
    for &byte in unit {
        bits.push(usize::from(byte), 8);
    }
    let mut packet = Vec::with_capacity(bits.bytes.len() + 3);
    packet.extend_from_slice(&[
        0x56,
        0xe0 | (bits.bytes.len() >> 8) as u8,
        bits.bytes.len() as u8,
    ]);
    packet.extend_from_slice(&bits.bytes);
    Ok(packet)
}

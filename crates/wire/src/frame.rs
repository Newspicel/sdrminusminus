//! Binary WS frame layout (PLAN §5). This is the *one* place a wire format is written by
//! hand on both sides (Rust encoder here, a ~100-line TS decoder in `web/`): the deliberate
//! exception to codegen (PLAN §4). All fields little-endian.
//!
//! ```text
//! header (16 bytes):
//!   u8  ver
//!   u8  kind            (FrameKind)
//!   u16 stream_id
//!   u32 seq
//!   u64 timestamp       (sample-count since capture start, PLAN §5)
//! SPECTRUM payload:
//!   f64 center_hz
//!   f32 span_hz
//!   f32 db_min
//!   f32 db_max
//!   u16 n
//!   u8[n] bins          (quantized magnitude over [db_min, db_max])
//! ```

/// Protocol version in every frame header. Bump on any layout change.
pub const PROTOCOL_VERSION: u8 = 1;

/// Byte length of the fixed frame header.
pub const HEADER_LEN: usize = 16;

/// Frame kinds (the `kind` header byte). Mirrored by the TS decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    Spectrum = 0,
    AudioOpus = 1,
    IqF32 = 2,
}

impl FrameKind {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Spectrum),
            1 => Some(Self::AudioOpus),
            2 => Some(Self::IqF32),
            _ => None,
        }
    }
}

/// A spectrum frame ready to encode. Bins are pre-quantized to `u8` over `[db_min, db_max]`
/// (PLAN §9: the adaptive dB window travels in the header).
#[derive(Clone, Debug, PartialEq)]
pub struct SpectrumFrame<'a> {
    pub stream_id: u16,
    pub seq: u32,
    pub timestamp: u64,
    pub center_hz: f64,
    pub span_hz: f32,
    pub db_min: f32,
    pub db_max: f32,
    pub bins: &'a [u8],
}

impl SpectrumFrame<'_> {
    /// Serialized length: header + fixed spectrum fields + bins.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 8 + 4 + 4 + 4 + 2 + self.bins.len()
    }

    /// Encode into a fresh little-endian byte buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.push(PROTOCOL_VERSION);
        buf.push(FrameKind::Spectrum as u8);
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.center_hz.to_le_bytes());
        buf.extend_from_slice(&self.span_hz.to_le_bytes());
        buf.extend_from_slice(&self.db_min.to_le_bytes());
        buf.extend_from_slice(&self.db_max.to_le_bytes());
        // n is bounded by the ≤4096-bin display cap (PLAN §9), so the cast never truncates.
        buf.extend_from_slice(&(self.bins.len() as u16).to_le_bytes());
        buf.extend_from_slice(self.bins);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode just enough to prove the layout matches the documented offsets.
    fn decode_spectrum(buf: &[u8]) -> (u8, FrameKind, u16, u32, u64, f64, f32, f32, f32, Vec<u8>) {
        let ver = buf[0];
        let kind = FrameKind::from_u8(buf[1]).expect("known kind");
        let stream_id = u16::from_le_bytes([buf[2], buf[3]]);
        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let timestamp = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let center_hz = f64::from_le_bytes(buf[16..24].try_into().unwrap());
        let span_hz = f32::from_le_bytes(buf[24..28].try_into().unwrap());
        let db_min = f32::from_le_bytes(buf[28..32].try_into().unwrap());
        let db_max = f32::from_le_bytes(buf[32..36].try_into().unwrap());
        let n = u16::from_le_bytes([buf[36], buf[37]]) as usize;
        let bins = buf[38..38 + n].to_vec();
        (
            ver, kind, stream_id, seq, timestamp, center_hz, span_hz, db_min, db_max, bins,
        )
    }

    #[test]
    fn spectrum_roundtrip() {
        let bins: Vec<u8> = (0..64u16).map(|i| (i * 4) as u8).collect();
        let frame = SpectrumFrame {
            stream_id: 7,
            seq: 42,
            timestamp: 1_000_000,
            center_hz: 100_300_000.0,
            span_hz: 2_400_000.0,
            db_min: -120.0,
            db_max: -20.0,
            bins: &bins,
        };
        let buf = frame.encode();
        assert_eq!(buf.len(), frame.encoded_len());

        let (ver, kind, sid, seq, ts, center, span, dmin, dmax, out) = decode_spectrum(&buf);
        assert_eq!(ver, PROTOCOL_VERSION);
        assert_eq!(kind, FrameKind::Spectrum);
        assert_eq!(sid, 7);
        assert_eq!(seq, 42);
        assert_eq!(ts, 1_000_000);
        assert_eq!(center, 100_300_000.0);
        assert_eq!(span, 2_400_000.0);
        assert_eq!(dmin, -120.0);
        assert_eq!(dmax, -20.0);
        assert_eq!(out, bins);
    }
}

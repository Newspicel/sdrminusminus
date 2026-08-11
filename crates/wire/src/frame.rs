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
//! AUDIO_OPUS payload:
//!   u8  ch_layout       (1 = mono, 2 = stereo — the packet's own Opus channel count)
//!   u8[] opus           (one Opus packet, to end of frame)
//! VIDEO_GRAY payload:
//!   u16 width
//!   u16 height
//!   u8[width·height] luma  (row-major from the top line, 0 = black)
//! ```
//!
//! AUDIO_OPUS timestamps count 48 kHz-domain sample *frames* since the channel's audio started
//! (PLAN §9: demods emit 48 kHz PCM before Opus encoding), so a layout change does not disturb
//! the clock a client detects loss on. `ch_layout` travels per frame because a channel may
//! switch layout mid-stream (WFM stereo toggled on a live channel).
//!
//! VIDEO_GRAY timestamps count channel-rate IQ samples since the channel started, so a gap
//! between pictures is legible as the time it really was. The picture is 8-bit luma and
//! uncompressed: it is one frame per field of an analog scan, sized by what the channel's
//! bandwidth resolved, and a codec between the demodulator and the canvas would cost more
//! than the bytes it saved on a desktop link.

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
    VideoGray = 3,
}

impl FrameKind {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Spectrum),
            1 => Some(Self::AudioOpus),
            2 => Some(Self::IqF32),
            3 => Some(Self::VideoGray),
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

/// An Opus audio frame ready to encode. The payload length is implicit: the opus packet
/// runs from byte `HEADER_LEN + 1` to the end of the WS frame.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioFrame<'a> {
    pub stream_id: u16,
    pub seq: u32,
    /// 48 kHz-domain sample-frame count since the channel's audio started.
    pub timestamp: u64,
    /// Channel layout of this packet; 1 = mono, 2 = stereo (interleaved L, R).
    pub ch_layout: u8,
    pub opus: &'a [u8],
}

impl AudioFrame<'_> {
    /// Serialized length: header + ch_layout + opus packet.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 1 + self.opus.len()
    }

    /// Encode into a fresh little-endian byte buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.push(PROTOCOL_VERSION);
        buf.push(FrameKind::AudioOpus as u8);
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.push(self.ch_layout);
        buf.extend_from_slice(self.opus);
        buf
    }
}

/// One decoded picture ready to encode: 8-bit luma, row-major, `width · height` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame<'a> {
    pub stream_id: u16,
    pub seq: u32,
    /// Channel-rate sample count when the picture completed.
    pub timestamp: u64,
    pub width: u16,
    pub height: u16,
    pub luma: &'a [u8],
}

impl VideoFrame<'_> {
    /// Serialized length: header + the geometry + one byte per pixel.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 2 + 2 + self.luma.len()
    }

    /// Encode into a fresh little-endian byte buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.push(PROTOCOL_VERSION);
        buf.push(FrameKind::VideoGray as u8);
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(self.luma);
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

    /// Decode just enough to prove the layout matches the documented offsets.
    fn decode_audio(buf: &[u8]) -> (u8, FrameKind, u16, u32, u64, u8, Vec<u8>) {
        let ver = buf[0];
        let kind = FrameKind::from_u8(buf[1]).expect("known kind");
        let stream_id = u16::from_le_bytes([buf[2], buf[3]]);
        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let timestamp = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let ch_layout = buf[16];
        let opus = buf[17..].to_vec();
        (ver, kind, stream_id, seq, timestamp, ch_layout, opus)
    }

    /// Both layouts travel through the same fixed offsets: only the `ch_layout` byte differs,
    /// so a stereo frame must not shift the opus payload by so much as a byte.
    #[test]
    fn audio_roundtrip_in_both_layouts() {
        let opus: Vec<u8> = (0..96u8).map(|i| i.wrapping_mul(3)).collect();
        for ch_layout in [1u8, 2] {
            let frame = AudioFrame {
                stream_id: 3,
                seq: 512,
                timestamp: 96_000,
                ch_layout,
                opus: &opus,
            };
            let buf = frame.encode();
            assert_eq!(buf.len(), frame.encoded_len());

            let (ver, kind, sid, seq, ts, layout, out) = decode_audio(&buf);
            assert_eq!(ver, PROTOCOL_VERSION);
            assert_eq!(kind, FrameKind::AudioOpus);
            assert_eq!(sid, 3);
            assert_eq!(seq, 512);
            assert_eq!(ts, 96_000);
            assert_eq!(layout, ch_layout);
            assert_eq!(out, opus);
        }
    }

    /// Decode just enough to prove the layout matches the documented offsets.
    fn decode_video(buf: &[u8]) -> (u8, FrameKind, u16, u32, u64, u16, u16, Vec<u8>) {
        let ver = buf[0];
        let kind = FrameKind::from_u8(buf[1]).expect("known kind");
        let stream_id = u16::from_le_bytes([buf[2], buf[3]]);
        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let timestamp = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let width = u16::from_le_bytes([buf[16], buf[17]]);
        let height = u16::from_le_bytes([buf[18], buf[19]]);
        let luma = buf[20..].to_vec();
        (ver, kind, stream_id, seq, timestamp, width, height, luma)
    }

    #[test]
    fn video_roundtrip() {
        let luma: Vec<u8> = (0..(8u32 * 4)).map(|i| (i * 7) as u8).collect();
        let frame = VideoFrame {
            stream_id: 0x8001,
            seq: 9,
            timestamp: 2_000_000,
            width: 8,
            height: 4,
            luma: &luma,
        };
        let buf = frame.encode();
        assert_eq!(buf.len(), frame.encoded_len());

        let (ver, kind, sid, seq, ts, width, height, out) = decode_video(&buf);
        assert_eq!(ver, PROTOCOL_VERSION);
        assert_eq!(kind, FrameKind::VideoGray);
        assert_eq!(sid, 0x8001);
        assert_eq!(seq, 9);
        assert_eq!(ts, 2_000_000);
        assert_eq!((width, height), (8, 4));
        assert_eq!(out, luma);
        // The payload length is what the geometry claims, so a client can size its ImageData
        // from the header alone.
        assert_eq!(out.len(), usize::from(width) * usize::from(height));
    }
}

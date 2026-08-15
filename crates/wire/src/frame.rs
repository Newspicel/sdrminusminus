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
    VideoRgb = 4,
}

impl FrameKind {
    #[must_use]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Spectrum),
            1 => Some(Self::AudioOpus),
            2 => Some(Self::IqF32),
            3 => Some(Self::VideoGray),
            4 => Some(Self::VideoRgb),
            _ => None,
        }
    }
}

/// A spectrum frame ready to encode. Bins are pre-quantized to `u8` over `[db_min, db_max]`
/// (: the adaptive dB window travels in the header).
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

/// A block of a channel's baseband IQ, ready to encode.
///
/// These are the samples a decoder actually sees: after the digital down-conversion and the
/// channel filter, at the channel's own rate, before the demodulator. Sent as contiguous bursts
/// rather than as a continuous stream — a constellation or an eye needs consecutive samples at the
/// full rate, and nothing that draws one needs every burst.
///
/// The payload length is implicit: interleaved I/Q pairs run from byte `HEADER_LEN + 12` to the
/// end of the WS frame, so the block is always an even number of `f32`s.
#[derive(Clone, Debug, PartialEq)]
pub struct IqFrame<'a> {
    pub stream_id: u16,
    pub seq: u32,
    /// Channel-rate sample count at the first sample of this block, so the gap between two
    /// bursts is legible as the time it really was.
    pub timestamp: u64,
    /// The channel's input rate, which is the bandwidth this baseband spans.
    pub sample_rate: f32,
    /// Absolute frequency the baseband is centred on: the device centre plus the channel offset.
    pub center_hz: f64,
    /// Interleaved I, Q.
    pub samples: &'a [f32],
}

impl IqFrame<'_> {
    /// Serialized length: header + rate + centre + four bytes per component.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 8 + 4 + self.samples.len() * 4
    }

    /// Encode into a fresh little-endian byte buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.push(PROTOCOL_VERSION);
        buf.push(FrameKind::IqF32 as u8);
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.center_hz.to_le_bytes());
        buf.extend_from_slice(&self.sample_rate.to_le_bytes());
        for sample in self.samples {
            buf.extend_from_slice(&sample.to_le_bytes());
        }
        buf
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VideoData<'a> {
    Gray(&'a [u8]),
    Rgb(&'a [u8]),
}

impl<'a> VideoData<'a> {
    fn kind(self) -> FrameKind {
        match self {
            Self::Gray(_) => FrameKind::VideoGray,
            Self::Rgb(_) => FrameKind::VideoRgb,
        }
    }

    fn bytes(self) -> &'a [u8] {
        match self {
            Self::Gray(bytes) | Self::Rgb(bytes) => bytes,
        }
    }
}

/// One decoded picture ready to encode: row-major 8-bit grayscale or RGB pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoFrame<'a> {
    pub stream_id: u16,
    pub seq: u32,
    /// Channel-rate sample count when the picture completed.
    pub timestamp: u64,
    pub width: u16,
    pub height: u16,
    pub data: VideoData<'a>,
}

impl VideoFrame<'_> {
    /// Serialized length: header + the geometry + one byte per pixel.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 2 + 2 + self.data.bytes().len()
    }

    /// Encode into a fresh little-endian byte buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.encoded_len());
        buf.push(PROTOCOL_VERSION);
        buf.push(self.data.kind() as u8);
        buf.extend_from_slice(&self.stream_id.to_le_bytes());
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.width.to_le_bytes());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(self.data.bytes());
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
    fn decode_iq(buf: &[u8]) -> (u8, FrameKind, u16, u32, u64, f64, f32, Vec<f32>) {
        let ver = buf[0];
        let kind = FrameKind::from_u8(buf[1]).expect("known kind");
        let stream_id = u16::from_le_bytes([buf[2], buf[3]]);
        let seq = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let timestamp = u64::from_le_bytes(buf[8..16].try_into().unwrap());
        let center_hz = f64::from_le_bytes(buf[16..24].try_into().unwrap());
        let sample_rate = f32::from_le_bytes(buf[24..28].try_into().unwrap());
        let (chunks, _) = buf[28..].as_chunks::<4>();
        let samples = chunks.iter().copied().map(f32::from_le_bytes).collect();
        (
            ver,
            kind,
            stream_id,
            seq,
            timestamp,
            center_hz,
            sample_rate,
            samples,
        )
    }

    #[test]
    fn iq_roundtrip() {
        let samples: Vec<f32> = (0..32).map(|i| i as f32 * 0.03125 - 0.5).collect();
        let frame = IqFrame {
            stream_id: 0x8100,
            seq: 3,
            timestamp: 48_000,
            sample_rate: 24_000.0,
            center_hz: 145_800_000.0,
            samples: &samples,
        };
        let buf = frame.encode();
        assert_eq!(buf.len(), frame.encoded_len());

        let (ver, kind, sid, seq, ts, center, rate, out) = decode_iq(&buf);
        assert_eq!(ver, PROTOCOL_VERSION);
        assert_eq!(kind, FrameKind::IqF32);
        assert_eq!(sid, 0x8100);
        assert_eq!(seq, 3);
        assert_eq!(ts, 48_000);
        assert_eq!(center, 145_800_000.0);
        assert_eq!(rate, 24_000.0);
        assert_eq!(out, samples);
        // Interleaved pairs: an odd component count would leave a reader one sample short of a
        // complex value and is not a frame this ever produces.
        assert_eq!(out.len() % 2, 0);
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
            data: VideoData::Gray(&luma),
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

    #[test]
    fn rgb_video_roundtrip() {
        let rgb: Vec<u8> = (0..(3 * 3 * 2)).map(|i| (i * 11) as u8).collect();
        let frame = VideoFrame {
            stream_id: 12,
            seq: 4,
            timestamp: 99,
            width: 3,
            height: 2,
            data: VideoData::Rgb(&rgb),
        };
        let buf = frame.encode();
        let (_, kind, _, _, _, width, height, out) = decode_video(&buf);
        assert_eq!(kind, FrameKind::VideoRgb);
        assert_eq!(out.len(), usize::from(width) * usize::from(height) * 3);
        assert_eq!(out, rgb);
    }
}

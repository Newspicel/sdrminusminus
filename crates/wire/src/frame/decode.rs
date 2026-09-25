use super::*;

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn open(buf: &'a [u8], kind: FrameKind) -> Option<(FrameHeader, Self)> {
        let header = FrameHeader::parse(buf)?;
        (header.version == PROTOCOL_VERSION && header.kind == kind).then_some((
            header,
            Self {
                buf,
                at: HEADER_LEN,
            },
        ))
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let taken = self.buf.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.array::<1>()?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.array()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.array()?))
    }

    fn rest(&mut self) -> &'a [u8] {
        let rest = self.buf.get(self.at..).unwrap_or_default();
        self.at = self.buf.len();
        rest
    }

    fn floats(&mut self, count: usize, out: &mut Vec<f32>) -> Option<()> {
        let bytes = self.take(count.checked_mul(4)?)?;
        out.extend(bytes.as_chunks::<4>().0.iter().map(|chunk| f32::from_le_bytes(*chunk)));
        Some(())
    }

    fn done(&self) -> bool {
        self.at == self.buf.len()
    }
}

impl<'a> IqFrame<'a> {
    #[must_use]
    pub fn decode(buf: &[u8], floats: &'a mut Vec<f32>) -> Option<Self> {
        let (header, mut reader) = Reader::open(buf, FrameKind::IqF32)?;
        let center_hz = reader.f64()?;
        let sample_rate = reader.f32()?;
        let body = reader.rest();
        if body.is_empty() || body.len() % 8 != 0 {
            return None;
        }
        floats.clear();
        Reader { buf: body, at: 0 }.floats(body.len() / 4, floats)?;
        Some(Self {
            stream_id: header.stream_id,
            seq: header.seq,
            timestamp: header.timestamp,
            center_hz,
            sample_rate,
            samples: floats,
        })
    }
}

impl<'a> SymbolFrame<'a> {
    #[must_use]
    pub fn decode(buf: &[u8], floats: &'a mut Vec<f32>) -> Option<Self> {
        let (header, mut reader) = Reader::open(buf, FrameKind::Symbols)?;
        let plane = SymbolPlane::from_u8(reader.u8()?)?;
        let symbol_rate = reader.f32()?;
        let evm = reader.f32()?;
        let mer_db = reader.f32()?;
        let margin = reader.f32()?;
        let freq_error_hz = reader.f32()?;
        let points = usize::from(reader.u16()?);
        floats.clear();
        reader.floats(points, floats)?;
        let body = reader.rest();
        if body.len() % 4 != 0 {
            return None;
        }
        Reader { buf: body, at: 0 }.floats(body.len() / 4, floats)?;
        let (reference, symbols) = floats.split_at(points);
        Some(Self {
            stream_id: header.stream_id,
            seq: header.seq,
            timestamp: header.timestamp,
            plane,
            symbol_rate,
            evm,
            mer_db,
            margin,
            freq_error_hz,
            reference,
            symbols,
        })
    }
}

impl<'a> RangeDopplerFrame<'a> {
    #[must_use]
    pub fn decode(buf: &'a [u8]) -> Option<Self> {
        let (header, mut reader) = Reader::open(buf, FrameKind::RangeDoppler)?;
        let ranges = reader.u16()?;
        let dopplers = reader.u16()?;
        let range_step_us = reader.f32()?;
        let doppler_step_hz = reader.f32()?;
        let db_min = reader.f32()?;
        let db_max = reader.f32()?;
        let cells = reader.take(usize::from(ranges) * usize::from(dopplers))?;
        if cells.is_empty() || !reader.done() {
            return None;
        }
        Some(Self {
            stream_id: header.stream_id,
            seq: header.seq,
            timestamp: header.timestamp,
            ranges,
            dopplers,
            range_step_us,
            doppler_step_hz,
            db_min,
            db_max,
            cells,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iq_decodes_what_it_encoded() {
        let samples = [0.25f32, -0.5, 1.0, 2.0];
        let frame = IqFrame {
            stream_id: 1,
            seq: 2,
            timestamp: 3,
            center_hz: 145.8e6,
            sample_rate: 48_000.0,
            samples: &samples,
        };
        let bytes = frame.encode();
        let mut floats = Vec::new();
        assert_eq!(IqFrame::decode(&bytes, &mut floats), Some(frame));
    }

    #[test]
    fn iq_refuses_an_odd_or_empty_burst() {
        let odd = IqFrame {
            stream_id: 1,
            seq: 0,
            timestamp: 0,
            center_hz: 1.0,
            sample_rate: 1.0,
            samples: &[1.0, 2.0, 3.0],
        }
        .encode();
        let mut floats = Vec::new();
        assert_eq!(IqFrame::decode(&odd, &mut floats), None);
        let empty = IqFrame {
            stream_id: 1,
            seq: 0,
            timestamp: 0,
            center_hz: 1.0,
            sample_rate: 1.0,
            samples: &[],
        }
        .encode();
        assert_eq!(IqFrame::decode(&empty, &mut floats), None);
    }

    #[test]
    fn symbols_decode_what_they_encoded() {
        let reference = [-3.0f32, -1.0, 1.0, 3.0];
        let symbols = [0.9f32, -1.1, 3.2];
        let frame = SymbolFrame {
            stream_id: 300,
            seq: 1,
            timestamp: 2,
            plane: SymbolPlane::Level,
            symbol_rate: 4800.0,
            evm: 0.125,
            mer_db: 18.0,
            margin: 2.5,
            freq_error_hz: -12.0,
            reference: &reference,
            symbols: &symbols,
        };
        let bytes = frame.encode();
        let mut floats = Vec::new();
        assert_eq!(SymbolFrame::decode(&bytes, &mut floats), Some(frame));
    }

    #[test]
    fn symbols_refuse_a_reference_that_runs_past_the_end() {
        let bytes = SymbolFrame {
            stream_id: 1,
            seq: 1,
            timestamp: 1,
            plane: SymbolPlane::Complex,
            symbol_rate: 1.0,
            evm: 0.0,
            mer_db: 0.0,
            margin: 0.0,
            freq_error_hz: 0.0,
            reference: &[1.0, 1.0],
            symbols: &[],
        }
        .encode();
        let mut floats = Vec::new();
        assert!(SymbolFrame::decode(&bytes, &mut floats).is_some());
        assert_eq!(SymbolFrame::decode(&bytes[..bytes.len() - 1], &mut floats), None);
    }

    #[test]
    fn a_surface_decodes_what_it_encoded_and_must_be_whole() {
        let cells = [1u8, 2, 3, 4, 5, 6];
        let frame = RangeDopplerFrame {
            stream_id: 9,
            seq: 1,
            timestamp: 1,
            ranges: 3,
            dopplers: 2,
            range_step_us: 1.0,
            doppler_step_hz: 2.0,
            db_min: -120.0,
            db_max: 0.0,
            cells: &cells,
        };
        let bytes = frame.encode();
        assert_eq!(RangeDopplerFrame::decode(&bytes), Some(frame));
        assert_eq!(RangeDopplerFrame::decode(&bytes[..bytes.len() - 1]), None);
        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(RangeDopplerFrame::decode(&longer), None);
    }

    #[test]
    fn a_decoder_refuses_another_kind() {
        let bytes = AudioFrame {
            stream_id: 1,
            seq: 1,
            timestamp: 1,
            ch_layout: 1,
            opus: &[1, 2, 3, 4, 5, 6, 7, 8],
        }
        .encode();
        let mut floats = Vec::new();
        assert_eq!(IqFrame::decode(&bytes, &mut floats), None);
        assert_eq!(SymbolFrame::decode(&bytes, &mut floats), None);
        assert_eq!(RangeDopplerFrame::decode(&bytes), None);
    }
}

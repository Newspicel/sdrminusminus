use sdrmm_wire::frame::{IqFrame, SymbolFrame, SymbolPlane};

#[derive(Clone, Debug, PartialEq)]
pub struct Burst {
    pub center_hz: f64,
    pub sample_rate: f32,
    pub samples: Vec<f32>,
}

impl Burst {
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let mut samples = Vec::new();
        let frame = IqFrame::decode(bytes, &mut samples)?;
        let (center_hz, sample_rate) = (frame.center_hz, frame.sample_rate);
        Some(Self {
            center_hz,
            sample_rate,
            samples,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub plane: SymbolPlane,
    pub symbol_rate: f32,
    pub evm: f32,
    pub mer_db: f32,
    pub margin: f32,
    pub freq_error_hz: f32,
    pub reference: Vec<f32>,
    pub symbols: Vec<f32>,
}

impl Block {
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let mut floats = Vec::new();
        let frame = SymbolFrame::decode(bytes, &mut floats)?;
        Some(Self {
            plane: frame.plane,
            symbol_rate: frame.symbol_rate,
            evm: frame.evm,
            mer_db: frame.mer_db,
            margin: frame.margin,
            freq_error_hz: frame.freq_error_hz,
            reference: frame.reference.to_vec(),
            symbols: frame.symbols.to_vec(),
        })
    }

    #[must_use]
    pub fn paired(&self) -> Vec<f32> {
        match self.plane {
            SymbolPlane::Complex => self.symbols.clone(),
            SymbolPlane::Level => self
                .symbols
                .iter()
                .flat_map(|level| [*level, 0.0])
                .collect(),
        }
    }

    #[must_use]
    pub fn reference_scale(&self) -> f32 {
        let peak = match self.plane {
            SymbolPlane::Complex => self
                .reference
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[re, im]| re.hypot(*im))
                .fold(0.0, f32::max),
            SymbolPlane::Level => self
                .reference
                .iter()
                .map(|level| level.abs())
                .fold(0.0, f32::max),
        };
        if peak > 0.0 { peak * 1.4 } else { 1.0 }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn block() -> Block {
        Block {
            plane: SymbolPlane::Level,
            symbol_rate: 4800.0,
            evm: 0.125,
            mer_db: 18.06,
            margin: 2.5,
            freq_error_hz: -12.0,
            reference: vec![1.0, 3.0, -1.0, -3.0],
            symbols: vec![1.0, -1.0, 3.0, -3.0],
        }
    }

    #[test]
    fn a_burst_and_a_block_are_read_off_the_wire() {
        let bytes = IqFrame {
            stream_id: 1,
            seq: 0,
            timestamp: 0,
            center_hz: 145.8e6,
            sample_rate: 48_000.0,
            samples: &[1.0, 0.0],
        }
        .encode();
        let burst = Burst::read(&bytes).expect("a burst");
        assert_eq!(burst.samples, vec![1.0, 0.0]);
        assert_eq!(burst.sample_rate, 48_000.0);
        let sent = block();
        let bytes = SymbolFrame {
            stream_id: 300,
            seq: 0,
            timestamp: 0,
            plane: sent.plane,
            symbol_rate: sent.symbol_rate,
            evm: sent.evm,
            mer_db: sent.mer_db,
            margin: sent.margin,
            freq_error_hz: sent.freq_error_hz,
            reference: &sent.reference,
            symbols: &sent.symbols,
        }
        .encode();
        assert_eq!(Block::read(&bytes), Some(sent));
        assert_eq!(Block::read(&bytes[..4]), None);
    }

    #[test]
    fn a_complex_cloud_stays_the_pairs_it_already_is() {
        let cloud = Block {
            plane: SymbolPlane::Complex,
            symbols: vec![0.7, 0.7, -0.7, 0.7],
            ..block()
        };
        assert_eq!(cloud.paired(), cloud.symbols);
    }

    #[test]
    fn a_level_rail_lies_along_the_real_axis() {
        assert_eq!(
            block().paired(),
            vec![1.0, 0.0, -1.0, 0.0, 3.0, 0.0, -3.0, 0.0]
        );
    }

    #[test]
    fn the_reference_scale_leaves_room_around_the_outermost_level() {
        assert!((block().reference_scale() - 3.0 * 1.4).abs() < 1e-5);
        let cloud = Block {
            plane: SymbolPlane::Complex,
            reference: vec![3.0, 4.0, -3.0, -4.0],
            ..block()
        };
        assert!((cloud.reference_scale() - 5.0 * 1.4).abs() < 1e-5);
        let bare = Block {
            reference: Vec::new(),
            ..block()
        };
        assert_eq!(bare.reference_scale(), 1.0);
    }
}

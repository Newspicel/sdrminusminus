use super::ldpc;

pub(crate) const SAMPLE_RATE: f32 = 12_000.0;
pub(crate) const MAX_SYMBOLS: usize = 103;
pub(crate) const MAX_TONES: usize = 8;
pub(crate) const BASEBAND_SYMBOL: usize = 32;

pub(crate) struct Protocol {
    pub(crate) symbol_samples: usize,
    pub(crate) tones: usize,
    pub(crate) bits_per_symbol: usize,
    pub(crate) symbols: usize,
    pub(crate) sync: &'static [(usize, &'static [u8])],
    pub(crate) gray: &'static [u8],
    pub(crate) scramble: u128,
    pub(crate) bt: f32,
    pub(crate) ramp_symbols: usize,
    pub(crate) slot_samples: usize,
    pub(crate) fft_samples: usize,
    pub(crate) earliest_start_s: f32,
    pub(crate) latest_start_s: f32,
    pub(crate) coherent_spans: &'static [usize],
    pub(crate) snr_offset_db: f32,
    pub(crate) min_sync: f32,
    pub(crate) coherent_sync: bool,
    pub(crate) full_time_search: bool,
    pub(crate) spectral_candidates: bool,
    pub(crate) osd_min_presence: f32,
}

const FT8_COSTAS: &[u8] = &[3, 1, 4, 0, 6, 5, 2];
const FT4_COSTAS: [&[u8]; 4] = [&[0, 1, 3, 2], &[1, 0, 2, 3], &[2, 3, 1, 0], &[3, 2, 0, 1]];

pub(crate) const FT8: Protocol = Protocol {
    symbol_samples: 1_920,
    tones: 8,
    bits_per_symbol: 3,
    symbols: 79,
    sync: &[(0, FT8_COSTAS), (36, FT8_COSTAS), (72, FT8_COSTAS)],
    gray: &[0, 1, 3, 2, 5, 6, 4, 7],
    scramble: 0,
    bt: 2.0,
    ramp_symbols: 0,
    slot_samples: 180_000,
    fft_samples: 192_000,
    earliest_start_s: -2.0,
    latest_start_s: 2.9,
    coherent_spans: &[1, 2, 3],
    snr_offset_db: -27.0,
    min_sync: 1.3,
    coherent_sync: false,
    full_time_search: false,
    spectral_candidates: true,
    osd_min_presence: 0.0,
};

pub(crate) const FT4: Protocol = Protocol {
    symbol_samples: 576,
    tones: 4,
    bits_per_symbol: 2,
    symbols: 103,
    sync: &[
        (0, FT4_COSTAS[0]),
        (33, FT4_COSTAS[1]),
        (66, FT4_COSTAS[2]),
        (99, FT4_COSTAS[3]),
    ],
    gray: &[0, 1, 3, 2],
    scramble: 0x4A5E_89B4_B08A_7955_BE28 >> 3,
    bt: 1.0,
    ramp_symbols: 1,
    slot_samples: 90_000,
    fft_samples: 97_200,
    earliest_start_s: -0.6,
    latest_start_s: 2.4,
    coherent_spans: &[1, 2, 4],
    snr_offset_db: -21.8,
    min_sync: 1.2,
    coherent_sync: true,
    full_time_search: true,
    spectral_candidates: true,
    osd_min_presence: 1.16,
};

impl Protocol {
    pub(crate) fn tone_spacing_hz(&self) -> f32 {
        SAMPLE_RATE / self.symbol_samples as f32
    }

    pub(crate) fn decimation(&self) -> usize {
        self.symbol_samples / BASEBAND_SYMBOL
    }

    pub(crate) fn baseband_rate(&self) -> f32 {
        SAMPLE_RATE / self.decimation() as f32
    }

    pub(crate) fn baseband_samples(&self) -> usize {
        self.fft_samples / self.decimation()
    }

    pub(crate) fn sync_tone(&self, symbol: usize) -> Option<u8> {
        self.sync.iter().find_map(|&(start, pattern)| {
            symbol
                .checked_sub(start)
                .and_then(|offset| pattern.get(offset).copied())
        })
    }

    pub(crate) fn data_blocks(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.sync.windows(2).map(|pair| {
            let start = pair[0].0 + pair[0].1.len();
            (start, pair[1].0 - start)
        })
    }

    pub(crate) fn inverse_gray(&self, tone: usize) -> usize {
        self.gray
            .iter()
            .position(|&mapped| usize::from(mapped) == tone)
            .unwrap_or(0)
    }

    pub(crate) fn tones_for(&self, payload: u128) -> [u8; MAX_SYMBOLS] {
        let codeword = ldpc::encode(payload ^ self.scramble);
        let mut tones = [0u8; MAX_SYMBOLS];
        let mut bits = codeword.chunks_exact(self.bits_per_symbol);
        for (symbol, tone) in tones.iter_mut().take(self.symbols).enumerate() {
            *tone = match self.sync_tone(symbol) {
                Some(sync) => sync,
                None => {
                    let value = bits.next().map_or(0, |chunk| {
                        chunk.iter().fold(0, |acc, &bit| (acc << 1) | bit)
                    });
                    self.gray[usize::from(value)]
                }
            };
        }
        tones
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_blocks_carry_the_whole_codeword() {
        for protocol in [&FT8, &FT4] {
            let symbols: usize = protocol.data_blocks().map(|(_, length)| length).sum();
            assert_eq!(symbols * protocol.bits_per_symbol, ldpc::N);
            let sync: usize = protocol.sync.iter().map(|(_, pattern)| pattern.len()).sum();
            assert_eq!(symbols + sync, protocol.symbols);
        }
    }
}

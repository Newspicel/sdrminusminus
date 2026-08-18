use std::sync::Arc;

use sdrmm_channels::SymbolTap;
use sdrmm_wire::SymbolPlane;

pub const SYMBOL_BLOCKS_PER_SEC: f64 = 10.0;
pub(crate) const SYMBOL_CHANNEL_CAP: usize = 4;
const MAX_SYMBOLS_PER_BLOCK: usize = 4096;

#[derive(Clone, Debug)]
pub struct SymbolBlock {
    pub seq: u32,
    pub timestamp: u64,
    pub plane: SymbolPlane,
    pub symbol_rate: f32,
    pub evm: f32,
    pub mer_db: f32,
    pub margin: f32,
    pub freq_error_hz: f32,
    pub reference: Arc<[f32]>,
    pub symbols: Arc<[f32]>,
}

pub(crate) struct SymbolBatcher {
    pending: Vec<f32>,
    reference: Vec<f32>,
    plane: Option<SymbolPlane>,
    symbol_rate: f32,
    evm: f32,
    mer_db: f32,
    margin: f32,
    freq_error_hz: f32,
    per_block: usize,
    seq: u32,
    position: u64,
}

impl SymbolBatcher {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            reference: Vec::new(),
            plane: None,
            symbol_rate: 0.0,
            evm: 0.0,
            mer_db: 0.0,
            margin: 0.0,
            freq_error_hz: 0.0,
            per_block: 0,
            seq: 0,
            position: 0,
        }
    }

    pub(crate) fn push(&mut self, tap: &SymbolTap, mut emit: impl FnMut(SymbolBlock)) {
        let Some(plane) = tap.plane else {
            return;
        };
        if self.plane != Some(plane) || self.reference != tap.reference {
            self.pending.clear();
            self.reference.clear();
            self.reference.extend_from_slice(&tap.reference);
            self.plane = Some(plane);
        }
        self.symbol_rate = tap.symbol_rate as f32;
        self.evm = tap.evm;
        self.mer_db = tap.mer_db;
        self.margin = tap.margin;
        self.freq_error_hz = tap.freq_error_hz;
        self.per_block = block_size(tap.symbol_rate, plane);
        self.pending.extend_from_slice(&tap.symbols);

        let width = usize::from(plane == SymbolPlane::Complex) + 1;
        let per_block = self.per_block * width;
        while self.pending.len() >= per_block && per_block > 0 {
            let block = SymbolBlock {
                seq: self.seq,
                timestamp: self.position,
                plane,
                symbol_rate: self.symbol_rate,
                evm: self.evm,
                mer_db: self.mer_db,
                margin: self.margin,
                freq_error_hz: self.freq_error_hz,
                reference: Arc::from(self.reference.as_slice()),
                symbols: Arc::from(&self.pending[..per_block]),
            };
            emit(block);
            self.seq = self.seq.wrapping_add(1);
            self.position += self.per_block as u64;
            self.pending.drain(..per_block);
        }
    }

    pub(crate) fn reset(&mut self) {
        self.pending.clear();
        self.reference.clear();
        self.plane = None;
    }
}

fn block_size(symbol_rate: f64, plane: SymbolPlane) -> usize {
    let _ = plane;
    let requested = (symbol_rate / SYMBOL_BLOCKS_PER_SEC) as usize;
    requested.clamp(1, MAX_SYMBOLS_PER_BLOCK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap(plane: SymbolPlane, symbols: Vec<f32>, rate: f64) -> SymbolTap {
        let mut tap = SymbolTap::default();
        tap.set_wanted(true);
        tap.plane = Some(plane);
        tap.symbols = symbols;
        tap.reference = vec![1.0, 3.0, -1.0, -3.0];
        tap.symbol_rate = rate;
        tap.margin = 2.5;
        tap
    }

    #[test]
    fn a_block_leaves_only_once_a_whole_cadence_of_symbols_has_arrived() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        let per_block = 480;

        batcher.push(&tap(SymbolPlane::Level, vec![1.0; 200], 4800.0), |b| {
            blocks.push(b);
        });
        assert!(blocks.is_empty(), "a partial block is not a block");

        batcher.push(&tap(SymbolPlane::Level, vec![1.0; 300], 4800.0), |b| {
            blocks.push(b);
        });
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].symbols.len(), per_block);
        assert_eq!(blocks[0].timestamp, 0);
        assert_eq!(blocks[0].seq, 0);
    }

    #[test]
    fn consecutive_blocks_carry_consecutive_stamps() {
        let mut batcher = SymbolBatcher::new();
        let mut stamps = Vec::new();
        for _ in 0..3 {
            batcher.push(&tap(SymbolPlane::Level, vec![1.0; 480], 4800.0), |b| {
                stamps.push((b.seq, b.timestamp));
            });
        }
        assert_eq!(stamps, vec![(0, 0), (1, 480), (2, 960)]);
    }

    #[test]
    fn a_complex_plane_block_counts_pairs_not_floats() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        batcher.push(
            &tap(SymbolPlane::Complex, vec![0.7; 2 * 480], 4800.0),
            |b| blocks.push(b),
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].symbols.len(), 2 * 480);
        assert_eq!(blocks[0].timestamp, 0);
    }

    #[test]
    fn a_changed_reference_starts_a_fresh_cloud_rather_than_splicing_two() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        batcher.push(&tap(SymbolPlane::Level, vec![1.0; 400], 4800.0), |b| {
            blocks.push(b);
        });

        let mut next = tap(SymbolPlane::Level, vec![1.0; 400], 4800.0);
        next.reference = vec![1.0, -1.0];
        batcher.push(&next, |b| blocks.push(b));
        assert!(blocks.is_empty(), "the old partial cloud survived a remap");

        batcher.push(&next, |b| blocks.push(b));
        assert_eq!(blocks.len(), 1);
        assert_eq!(&*blocks[0].reference, &[1.0, -1.0]);
    }

    #[test]
    fn a_tap_with_nothing_in_it_emits_nothing() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        batcher.push(&SymbolTap::default(), |b| blocks.push(b));
        assert!(blocks.is_empty());
    }

    #[test]
    fn a_slow_mode_still_batches_at_least_one_symbol() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        batcher.push(&tap(SymbolPlane::Complex, vec![0.5; 8], 31.25), |b| {
            blocks.push(b);
        });
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].symbols.len(), 2 * 3);
    }

    #[test]
    fn a_reset_drops_the_partial_block_rather_than_splicing_over_the_gap() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        batcher.push(&tap(SymbolPlane::Level, vec![1.0; 400], 4800.0), |b| {
            blocks.push(b);
        });
        batcher.reset();
        batcher.push(&tap(SymbolPlane::Level, vec![2.0; 480], 4800.0), |b| {
            blocks.push(b);
        });
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].symbols.iter().all(|&s| s == 2.0));
    }

    #[test]
    fn the_measurement_that_rides_along_is_the_one_the_decoder_just_made() {
        let mut batcher = SymbolBatcher::new();
        let mut blocks = Vec::new();
        let mut source = tap(SymbolPlane::Level, vec![1.0; 480], 4800.0);
        source.mer_db = 17.5;
        source.freq_error_hz = -42.0;
        batcher.push(&source, |b| blocks.push(b));
        assert_eq!(blocks[0].mer_db, 17.5);
        assert_eq!(blocks[0].margin, 2.5);
        assert_eq!(blocks[0].freq_error_hz, -42.0);
        assert_eq!(blocks[0].symbol_rate, 4800.0);
    }
}

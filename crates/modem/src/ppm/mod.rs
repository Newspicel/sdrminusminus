mod demod;
mod grid;
mod modulator;

pub use demod::{MAX_SLOT_BITS, MAX_SLOTS, PpmDemod, SlotDetector, llrs, soft_bits};
pub use grid::{SlotGrid, magnitudes};
pub use modulator::{OVERSAMPLE, PpmMod, SlotWaveform, slot_taps};

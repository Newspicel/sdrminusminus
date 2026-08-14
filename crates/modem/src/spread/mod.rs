//! Spread spectrum (MODEM-PLAN §3.1 `spread/`, §7 phase 7): the waveforms that occupy far more
//! bandwidth than their data rate needs, and the four different things they buy with it.
//!
//! **A framework is not a peer of a mapper** (§3.3). [`dsss`] and [`fhss`] carry a
//! constellation's points and hand them back; `constellation/` supplies the table and the one
//! demapper. [`cck`] and [`css`] are block codes over chips rather than point sets, so they
//! demap through the crate's *other* shared demapper — the energy one the orthogonal engine
//! uses — and neither owns a mapping table of its own.
//!
//! What separates the four, stated once so the catalog rows do not have to repeat it:
//!
//! | Entry | Bandwidth buys | Reference |
//! |---|---|---|
//! | [`dsss`] | rejection of narrowband interference, `10·log₁₀(N)` | its constellation's own oracle |
//! | [`cck`] | rate — 8 chips carry 8 bits at 802.11b's 11 Mbit/s | committed |
//! | [`css`] | sensitivity — `2^SF` orthogonal signals, `SF` bits each | the noncoherent orthogonal oracle |
//! | [`fhss`] | escape — a jammer parked on one channel misses the rest | its underlying entry's curve |
//!
//! **Under AWGN, spreading is transparent.** That is the acceptance the first and last rows are
//! held to and it is worth being blunt about, because a spread-spectrum entry that measured
//! *better* than its unspread twin under thermal noise would be measuring a harness defect: a
//! chip carries `1/N` of the symbol's energy and the correlator collects `N` of them, so the
//! ratio is unchanged and every dB a committed curve sits from its oracle is framing. What
//! spreading is worth appears on the *interference* axes instead, and this phase measures it
//! there — as the correlator's own input-to-output SIR, and as the C/I two spreading factors fail
//! at.
//!
//! **CSS is the third member of an identity phase 5 started.** M tones in one interval and M
//! intervals at one tone are the same signalling set; so are M cyclic shifts of one chirp, since
//! dechirping turns them into the M columns of a DFT. So the chirp entry answers to the same
//! exact closed form the M-FSK filterbank and the M-PPM matched filter do
//! ([`theory::mfsk_noncoherent_ser`](crate::ber::theory::mfsk_noncoherent_ser)), at `M = 2^SF` —
//! which is what put a large-`M` evaluation of that oracle in the harness.
//!
//! **No protocol attachments** (§6 scope decision): Barker-11 DSSS, the CCK codebooks and
//! LoRa-like CSS parameterisations are exercised as modulation entries on synthetic vectors only.
//! No PLCP, no SIGNAL field, no LoRa preamble or header, no `wifi`/`lora` channel.

pub mod cck;
pub mod chip;
pub mod css;
pub mod dsss;
pub mod fhss;
pub mod pn;

pub use cck::{CckDemod, CckMod, CckMode, CckParams, Codebook};
pub use chip::{ChipShaper, find_burst};
pub use css::{CssDemod, CssMod, CssParams, MAX_SPREADING_FACTOR, MIN_SPREADING_FACTOR};
pub use dsss::{Acquisition, DsssDemod, DsssMod, DsssParams, MAX_CHIPS};
pub use fhss::{FhssDemod, FhssMod, HopSequence, HopSequencer};
pub use pn::{MAX_LFSR_DEGREE, PnError, PnSequence};

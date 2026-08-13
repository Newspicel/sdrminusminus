//! Analog modulation (MODEM-PLAN §3.1 `analog/`, §7 phase 8): the four families that carry a
//! waveform rather than a symbol, and the seven detectors that read them back.
//!
//! **What makes an analog entry an entry.** Every other row in the catalog is judged by counting
//! errors; there is nothing here to count. §5 item 4 states the substitution — SINAD and THD
//! against input SNR in place of BER against Eb/N0 — and [`ber::analog`](crate::ber::analog)
//! implements it, with the same seeds, the same committed JSON, and the same one-sided
//! comparators. Nothing else about the regime changes.
//!
//! **Every row has a closed form.** That is not what "analog" suggests, and it is the phase's
//! most useful structural fact: above its detector's threshold each family's output SINAD is its
//! input channel SNR plus a constant that depends only on the modulation's own geometry — the
//! **figure of merit** ([`theory`](crate::ber::theory)). So the analog rows are oracle-matched
//! like the linear ones, not commit-and-guard, and what is committed instead is the *knee*: the
//! SNR at which each detector's nonlinearity takes over and the straight line ends.
//!
//! | Family | Module | Figure of merit | Detectors |
//! |---|---|---|---|
//! | AM, full carrier | [`am`] | `(m²P̄)/(1+m²P̄)` — ⅓ at full depth | envelope, synchronous |
//! | DSB-SC, VSB | [`am`] | 1 | synchronous (envelope on VSB) |
//! | SSB (USB/LSB) | [`ssb`] | 1 | filter, Weaver |
//! | FM | [`angle`] | `3β²P̄` | discriminator, PLL |
//! | PM | [`angle`] | `β_p²P̄` | argument, PLL |
//!
//! Read down the third column and the whole family tree is there: **amplitude modulation cannot
//! beat unity and angle modulation is unbounded**, because the first spends bandwidth on a
//! mirror image and the second spends it on deviation, which enters squared. What a sideband
//! buys is spectrum, not sensitivity; what a deviation buys is sensitivity, at a threshold that
//! arrives sooner the more of it you buy.
//!
//! **The three optional stages.** Each receiver carries a predetection band filter, a
//! post-detection audio filter and a DC blocker, and each can be turned off. That is not
//! configurability for its own sake — it is what lets the repo's five analog channels sit on
//! these engines without paying for a filter their runtime already applied, and what lets a
//! video consumer read a detector's raw wideband output where an audio one wants it band-limited
//! (`channels::atv` is the case, and its blanking level is exactly the DC a voice channel must
//! remove).
//!
//! **Why the predetection filter is part of the measurement.** An analog detector is nonlinear,
//! so noise outside the transmitted band does not pass through it — it folds down into the audio
//! and is counted twice. The IF filter a real receiver has is what makes the closed forms above
//! apply at all, which is why the engines carry one rather than assuming their input arrived
//! clean.

pub mod am;
pub mod angle;
pub mod filter;
pub mod ssb;

pub use am::{AmDemod, AmDetector, AmMod, AmMode, AmParams, AmRx};
pub use angle::{AngleDemod, AngleDetector, AngleKind, AngleMod, AngleParams, AngleRx};
pub use filter::{BandFilter, Delay, design_hilbert, design_vestigial};
pub use ssb::{Sideband, SsbDemod, SsbDetector, SsbMethod, SsbMod, SsbParams};

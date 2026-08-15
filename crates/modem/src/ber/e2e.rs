use std::{error::Error, fmt};

use super::{
    impair::{Awgn, Channel, ChannelSpec, Impairment},
    rng::Rng,
    sweep::Link,
};

pub struct Payload {
    pub seed: u64,
    pub bits: Vec<bool>,
    channel_rng: Rng,
}

impl Payload {
    #[must_use]
    pub fn from_seed(seed: u64, bit_count: usize) -> Self {
        let mut rng = Rng::new(seed);
        let bits = random_bits(&mut rng, bit_count);
        Self {
            seed,
            bits,
            channel_rng: rng,
        }
    }
}

fn random_bits(rng: &mut Rng, n: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(n);
    while bits.len() < n {
        let mut word = rng.next_u64();
        let take = 64.min(n - bits.len());
        for _ in 0..take {
            bits.push(word & 1 == 1);
            word >>= 1;
        }
    }
    bits
}

#[derive(Clone, Debug)]
pub struct Payloads {
    seed: u64,
    bits_per_payload: usize,
    count: usize,
    produced: usize,
}

impl Payloads {
    #[must_use]
    pub fn new(seed: u64, count: usize, bits_per_payload: usize) -> Self {
        Self {
            seed,
            bits_per_payload,
            count,
            produced: 0,
        }
    }
}

impl Iterator for Payloads {
    type Item = Payload;

    fn next(&mut self) -> Option<Payload> {
        if self.produced == self.count {
            return None;
        }
        let stride = (self.produced as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.produced += 1;
        Some(Payload::from_seed(
            self.seed.wrapping_add(stride),
            self.bits_per_payload,
        ))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.count - self.produced;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Payloads {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub payload_index: usize,
    pub payload_seed: u64,
    pub first_bit: usize,
    pub bit_errors: u64,
    pub payload_bits: usize,
    pub decoded_bits: usize,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "payload {} (seed {:#x}): {}/{} bits differ, first at bit {}, \
             demodulator returned {} bits",
            self.payload_index,
            self.payload_seed,
            self.bit_errors,
            self.payload_bits,
            self.first_bit,
            self.decoded_bits
        )
    }
}

impl Error for Mismatch {}

pub fn loopback(
    link: &mut Link,
    channel: &mut dyn Impairment,
    payloads: impl IntoIterator<Item = Payload>,
) -> Result<(), Mismatch> {
    for (payload_index, payload) in payloads.into_iter().enumerate() {
        let Payload {
            seed,
            bits,
            mut channel_rng,
        } = payload;
        let mut wave = (link.modulate)(&bits);
        channel.apply(&mut wave, &mut channel_rng);
        let decoded = (link.demodulate)(&wave);

        let mut first_bit = None;
        let mut bit_errors = 0u64;
        for (i, &sent) in bits.iter().enumerate() {
            if decoded.get(i) != Some(&sent) {
                bit_errors += 1;
                if first_bit.is_none() {
                    first_bit = Some(i);
                }
            }
        }
        if let Some(first_bit) = first_bit {
            return Err(Mismatch {
                payload_index,
                payload_seed: seed,
                first_bit,
                bit_errors,
                payload_bits: bits.len(),
                decoded_bits: decoded.len(),
            });
        }
    }
    Ok(())
}

#[must_use]
pub fn channel_at_margin(
    template: &ChannelSpec,
    link: &Link,
    sensitivity_1e3_db: f64,
    margin_db: f64,
) -> Channel {
    template
        .awgn(Awgn::for_ebn0(
            sensitivity_1e3_db + margin_db,
            link.bits_per_trial as u64,
        ))
        .build()
}

#[cfg(test)]
mod tests {
    use std::iter;

    use super::*;
    use crate::ber::{reference::ideal_bpsk, theory};

    fn ebn0_at_ber(oracle: impl Fn(f64) -> f64, ber: f64) -> f64 {
        let (mut lo, mut hi) = (-10.0f64, 20.0f64);
        assert!(oracle(lo) >= ber && oracle(hi) <= ber);
        while hi - lo > 1e-9 {
            let mid = 0.5 * (lo + hi);
            if oracle(mid) >= ber {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    fn with_demod(link: Link, f: impl Fn(Vec<bool>) -> Vec<bool> + 'static) -> Link {
        let inner = link.demodulate;
        Link {
            label: format!("{} (broken)", link.label),
            bits_per_trial: link.bits_per_trial,
            modulate: link.modulate,
            demodulate: Box::new(move |wave| f(inner(wave))),
        }
    }

    #[test]
    fn ideal_bpsk_loops_back_clean_at_6db_margin() {
        let mut link = ideal_bpsk();
        let sensitivity = ebn0_at_ber(theory::bpsk_ber, 1e-3);
        let payloads = Payloads::new(0x00e2e, 20, link.bits_per_trial);
        let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, 6.0);
        assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
    }

    #[test]
    fn inverted_link_reports_first_bit_zero_and_every_bit_counted() {
        let mut link = with_demod(ideal_bpsk(), |bits| bits.into_iter().map(|b| !b).collect());
        let mut channel = ChannelSpec::default().build();
        let n = 512;
        let err = loopback(&mut link, &mut channel, Payloads::new(1, 3, n)).unwrap_err();
        assert_eq!(err.payload_index, 0);
        assert_eq!(err.first_bit, 0);
        assert_eq!(err.bit_errors, n as u64);
        assert_eq!(err.payload_bits, n);
        assert_eq!(err.decoded_bits, n);
    }

    #[test]
    fn single_flipped_bit_is_located_exactly() {
        let mut link = with_demod(ideal_bpsk(), |mut bits| {
            bits[137] = !bits[137];
            bits
        });
        let mut channel = ChannelSpec::default().build();
        let err = loopback(&mut link, &mut channel, Payloads::new(2, 1, 512)).unwrap_err();
        assert_eq!(err.first_bit, 137);
        assert_eq!(err.bit_errors, 1);
        assert_eq!(err.decoded_bits, 512);
    }

    #[test]
    fn truncated_demodulator_counts_missing_bits_as_errors() {
        let mut link = with_demod(ideal_bpsk(), |mut bits| {
            bits.truncate(100);
            bits
        });
        let mut channel = ChannelSpec::default().build();
        let err = loopback(&mut link, &mut channel, Payloads::new(3, 1, 512)).unwrap_err();
        assert_eq!(err.first_bit, 100);
        assert_eq!(err.bit_errors, 412);
        assert_eq!(err.payload_bits, 512);
        assert_eq!(err.decoded_bits, 100);
    }

    #[test]
    fn same_seeds_reproduce_the_identical_mismatch() {
        let sensitivity = ebn0_at_ber(theory::bpsk_ber, 1e-3);
        let run = || {
            let mut link = ideal_bpsk();
            let payloads = Payloads::new(0xd37, 4, link.bits_per_trial);
            let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, -3.0);
            loopback(&mut link, &mut channel, payloads)
        };
        let a = run().unwrap_err();
        let b = run().unwrap_err();
        assert_eq!(a, b);

        let mut link = ideal_bpsk();
        let bit_count = link.bits_per_trial;
        let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, -3.0);
        let solo = loopback(
            &mut link,
            &mut channel,
            iter::once(Payload::from_seed(a.payload_seed, bit_count)),
        )
        .unwrap_err();
        assert_eq!(solo.payload_index, 0, "solo reproduction is its own run");
        assert_eq!(solo.payload_seed, a.payload_seed);
        assert_eq!(solo.first_bit, a.first_bit);
        assert_eq!(solo.bit_errors, a.bit_errors);
    }
}

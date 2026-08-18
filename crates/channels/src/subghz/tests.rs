use sdrmm_wire::NfmParams;

use super::*;
use crate::{
    testgen::{
        self,
        subghz::{
            Ppm, Pwm, PwmBurst, fsk_nrz, keyed, manchester, ppm_timings, pwm, pwm_burst_timings,
        },
    },
    testutil::{complex_noise, settings},
};

const RATE: f64 = 250_000.0;
const BLOCKS: [usize; 7] = [997, 1, 4_096, 65, 2_048, 7, 1_024];

fn channel(p: SubghzParams) -> SubghzChannel {
    SubghzChannel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Subghz(p)),
    )
    .unwrap()
}

fn decode_blocks(
    chan: &mut SubghzChannel,
    iq: &[Complex<f32>],
    lens: &[usize],
) -> Vec<SubghzFrame> {
    let mut out = ChannelOutputs::default();
    let mut frames = Vec::new();
    let mut pos = 0;
    for len in lens.iter().cycle() {
        if pos >= iq.len() {
            break;
        }
        let end = (pos + len).min(iq.len());
        out.reset();
        chan.process(&iq[pos..end], &mut out);
        assert!(out.audio_pcm.is_empty(), "subghz must not produce audio");
        for ev in &out.events {
            match ev {
                DecoderEvent::Subghz(f) => frames.push(f.clone()),
                other => panic!("unexpected event {other:?}"),
            }
        }
        pos = end;
    }
    frames
}

fn decode(p: SubghzParams, iq: &[Complex<f32>]) -> Vec<SubghzFrame> {
    decode_blocks(&mut channel(p), iq, &BLOCKS)
}

const REMOTE: u32 = 0x0A_1B_23;

fn ev1527() -> Pwm {
    Pwm {
        bits: (0..24).map(|i| REMOTE >> (23 - i) & 1 == 1).collect(),
        short_us: 320,
        long_multiple: 3,
        sync_gap_multiple: 31,
        repeats: 6,
    }
}

#[test]
fn decodes_an_ook_remote_and_collapses_its_repeats() {
    let frames = decode(SubghzParams::default(), &pwm(&ev1527(), RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    let f = &frames[0];
    assert_eq!(f.modulation, SubghzModulation::Ook);
    assert_eq!(f.encoding, SubghzEncoding::Pwm);
    assert_eq!(f.bits, 24);
    assert_eq!(f.data, "0A1B23");
    assert_eq!(f.address, Some(REMOTE >> 4));
    assert_eq!(f.button, Some((REMOTE & 0xF) as u8));
    assert!(
        (300..=340).contains(&f.short_us),
        "base period {} µs",
        f.short_us
    );
    assert!(f.repeats >= 5, "collapsed {} of 6 repeats", f.repeats);
}

#[test]
fn tri_state_is_offered_only_when_every_pair_is_a_symbol() {
    let all_symbols: Vec<bool> = [
        [false, false],
        [true, true],
        [false, true],
        [false, false],
        [true, true],
        [false, true],
        [false, false],
        [true, true],
        [false, true],
        [false, false],
        [true, true],
        [false, true],
    ]
    .concat();
    assert_eq!(tri_state(&all_symbols).as_deref(), Some("01F01F01F01F"));
    let mut with_ten = all_symbols.clone();
    with_ten[0] = true;
    assert_eq!(tri_state(&with_ten), None);
    assert_eq!(tri_state(&all_symbols[..20]), None, "wrong length");
}

#[test]
fn decodes_a_manchester_sensor() {
    let bits: Vec<bool> = (0..32)
        .map(|i| (0xC3A5_96F0u32 >> (31 - i)) & 1 == 1)
        .collect();
    let frames = decode(SubghzParams::default(), &manchester(&bits, 250, 4, RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].encoding, SubghzEncoding::Manchester);
    assert_eq!(frames[0].bits, 32);
    assert_eq!(frames[0].data, "C3A596F0");
}

#[test]
fn decodes_an_fsk_remote() {
    let p = SubghzParams {
        modulation: SubghzModulation::Fsk,
        ..SubghzParams::default()
    };
    let frames = decode(p, &testgen::subghz::pwm_fsk(&ev1527(), 40_000.0, RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].modulation, SubghzModulation::Fsk);
    assert_eq!(frames[0].data, "0A1B23");
}

#[test]
fn an_unrecognised_burst_is_reported_as_raw_timings() {
    let odd = testgen::subghz::keyed(&[900, 400, 300, 1_700, 250, 260, 1_100, 380, 700], RATE);
    let frames = decode(SubghzParams::default(), &odd);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].encoding, SubghzEncoding::Raw);
    assert_eq!(frames[0].bits, 0);
    assert!(frames[0].data.is_empty());
    assert!(
        frames[0].timings_us.len() >= 8,
        "timings {:?}",
        frames[0].timings_us
    );
}

#[test]
fn a_fragment_is_superseded_by_the_whole_frames_behind_it() {
    let remote = ev1527();
    let full = pwm(&remote, RATE);
    let frame_us: u32 = testgen::subghz::pwm_timings(&remote).iter().sum();
    let cut = (0.05 * RATE) as usize + (1.5 * f64::from(frame_us) * 1e-6 * RATE) as usize;
    let frames = decode(SubghzParams::default(), &full[cut..]);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].data, "0A1B23");
    assert_eq!(frames[0].bits, 24);
}

#[test]
fn decodes_through_additive_noise() {
    let mut iq = pwm(&ev1527(), RATE);
    testgen::add_noise(&mut iq, 0xabad_1dea, 0.1);
    let mut filtered = Vec::new();
    channel_filter(&SubghzParams::default())
        .unwrap()
        .process(&iq, &mut filtered);
    let frames = decode(SubghzParams::default(), &filtered);
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].data, "0A1B23");
}

#[test]
fn pure_noise_decodes_to_nothing() {
    for seed in [0x1234_5678, 0xdead_beef, 0x0f0f_0f0f] {
        let noise = complex_noise(seed, 0.05, 1_000_000);
        assert_eq!(
            decode(SubghzParams::default(), &noise),
            Vec::new(),
            "seed {seed:#x}"
        );
    }
}

#[test]
fn ragged_block_splits_decode_identically() {
    let iq = pwm(&ev1527(), RATE);
    let whole = decode_blocks(&mut channel(SubghzParams::default()), &iq, &[iq.len()]);
    let ragged = decode_blocks(&mut channel(SubghzParams::default()), &iq, &BLOCKS);
    let single = decode_blocks(&mut channel(SubghzParams::default()), &iq, &[1]);
    assert_eq!(whole.len(), 1);
    assert_eq!(ragged, whole);
    assert_eq!(single, whole);
}

#[test]
fn retune_drops_the_frame_being_held() {
    let iq = pwm(&ev1527(), RATE);
    let mut chan = channel(SubghzParams::default());
    let held = iq.len() - (0.4 * RATE) as usize;
    assert!(decode_blocks(&mut chan, &iq[..held], &BLOCKS).is_empty());
    chan.retuned();
    assert_eq!(
        decode_blocks(&mut chan, &iq[held..], &BLOCKS),
        Vec::new(),
        "a frame from the frequency we left must not be emitted here"
    );
}

#[test]
fn base_period_ignores_one_clipped_edge() {
    let edges = [80, 79, 81, 240, 80, 78, 82, 240, 80, 80];
    let base = base_period(&edges).unwrap();
    assert!((78..=82).contains(&base), "base {base}");
    assert_eq!(multiple(240, base), Some(3));
    assert_eq!(multiple(80, base), Some(1));
    assert_eq!(multiple(120, base), None, "1.5× is not a whole multiple");
}

#[test]
fn hex_pads_to_whole_nibbles() {
    assert_eq!(hex_of(&[true; 4]), "F");
    assert_eq!(hex_of(&[true, false, true]), "5");
    assert_eq!(hex_of(&[]), "");
}

#[test]
fn out_of_range_params_are_rejected() {
    for p in [
        SubghzParams {
            bandwidth_hz: 0.0,
            ..SubghzParams::default()
        },
        SubghzParams {
            bandwidth_hz: f64::NAN,
            ..SubghzParams::default()
        },
        SubghzParams {
            bandwidth_hz: 240_000.0,
            ..SubghzParams::default()
        },
        SubghzParams {
            min_pulse_us: 0,
            ..SubghzParams::default()
        },
        SubghzParams {
            min_pulse_us: 9_000,
            ..SubghzParams::default()
        },
    ] {
        assert!(
            matches!(channel_filter(&p), Err(ChannelError::InvalidSettings(_))),
            "{p:?} must be rejected"
        );
        assert!(matches!(
            SubghzChannel::new(
                ChannelCtx { input_rate: RATE },
                settings(ChannelParams::Subghz(p)),
            ),
            Err(ChannelError::InvalidSettings(_))
        ));
    }
}

#[test]
fn mismatched_params_variant_is_rejected() {
    let mut chan = channel(SubghzParams::default());
    let err = chan.apply(settings(ChannelParams::Nfm(NfmParams::default())));
    assert!(matches!(err, Err(ChannelError::InvalidSettings(_))));
}

#[test]
fn wrong_input_rate_is_rejected() {
    let built = SubghzChannel::new(
        ChannelCtx {
            input_rate: 48_000.0,
        },
        settings(ChannelParams::Subghz(SubghzParams::default())),
    );
    assert!(matches!(built, Err(ChannelError::InvalidSettings(_))));
}

fn payload_bits(bytes: &[u8], count: usize) -> Vec<bool> {
    (0..count)
        .map(|index| bytes[index / 8] >> (7 - index % 8) & 1 == 1)
        .collect()
}

fn nexus_burst(payload: &[u8; 5], repeats: u32) -> Ppm {
    Ppm {
        bits: payload_bits(payload, 36),
        pulse_us: 500,
        short_gap_us: 1_000,
        long_gap_us: 2_000,
        sync_gap_us: 4_000,
        repeats,
    }
}

#[test]
fn reads_a_nexus_weather_sensor_off_a_repeated_ppm_burst() {
    let burst = nexus_burst(&[0x8F, 0x80, 0xD5, 0xF2, 0xF0], 8);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    let f = &frames[0];
    assert_eq!(f.encoding, SubghzEncoding::Ppm);
    assert_eq!(f.bits, 36);
    assert_eq!(f.data, "8F80D5F2F");
    let reading = f.reading.as_ref().expect("a sensor reading");
    assert_eq!(reading.model, "Nexus-TH");
    assert_eq!(reading.id, 0x8F);
    assert_eq!(reading.channel, Some(1));
    assert_eq!(reading.battery_ok, Some(true));
    assert_eq!(reading.temperature_c, Some(21.3));
    assert_eq!(reading.humidity_pct, Some(47.0));
    assert_eq!(f.repeats, 8);
}

#[test]
fn reads_a_lacrosse_sensor_through_the_preamble_of_every_packet() {
    let burst = PwmBurst {
        bits: payload_bits(&[0x2A, 0xA1, 0xA9, 0x58, 0x23], 40),
        short_us: 208,
        long_us: 417,
        preamble_us: 833,
        preamble_pulses: 4,
        repeats: 10,
    };
    let frames = decode(
        SubghzParams::default(),
        &keyed(&pwm_burst_timings(&burst), RATE),
    );
    assert_eq!(frames.len(), 1, "{frames:?}");
    let reading = frames[0].reading.as_ref().expect("a sensor reading");
    assert_eq!(reading.model, "LaCrosse-TX141THBv2");
    assert_eq!(reading.temperature_c, Some(-7.5));
    assert_eq!(reading.humidity_pct, Some(88.0));
    assert_eq!(reading.channel, Some(2));
    assert_eq!(reading.battery_ok, Some(false));
}

#[test]
fn collapses_acurite_packets_that_arrive_as_separate_frames() {
    let burst = Ppm {
        bits: payload_bits(&[0xD4, 0xA0, 0xF6, 0x3D, 0xA7], 40),
        pulse_us: 500,
        short_gap_us: 1_000,
        long_gap_us: 2_000,
        sync_gap_us: 8_000,
        repeats: 3,
    };
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    let f = &frames[0];
    assert_eq!(f.repeats, 3);
    let reading = f.reading.as_ref().expect("a sensor reading");
    assert_eq!(reading.model, "Acurite-609TXC");
    assert_eq!(reading.temperature_c, Some(24.6));
    assert_eq!(reading.humidity_pct, Some(61.0));
    assert_eq!(reading.battery_ok, Some(false));
}

#[test]
fn reads_a_sensor_that_arrives_through_additive_noise() {
    let burst = nexus_burst(&[0x5C, 0x2F, 0xB5, 0xF5, 0x80], 8);
    let mut iq = keyed(&ppm_timings(&burst), RATE);
    let noise = complex_noise(1 << 20, 0.05, iq.len());
    for (sample, noise) in iq.iter_mut().zip(noise) {
        *sample += noise;
    }
    let frames = decode(SubghzParams::default(), &iq);
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.temperature_c, Some(-7.5));
    assert_eq!(reading.humidity_pct, Some(88.0));
}

#[test]
fn a_payload_that_fails_its_checksum_is_never_reported_as_a_reading() {
    let burst = nexus_burst(&[0x8F, 0x80, 0xD5, 0xE2, 0xF0], 8);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    assert!(
        frames.iter().all(|f| f.reading.is_none()),
        "a broken constant nibble was read as a sensor: {frames:?}"
    );
}

#[test]
fn a_sensor_heard_in_two_bursts_reports_every_repeat_it_carried() {
    let payload = [0x8F, 0x80, 0xD5, 0xF2, 0xF0];
    let mut first = ppm_timings(&nexus_burst(&payload, 4));
    let last = first.len() - 1;
    first[last] = 20_000;
    let mut timings = first;
    timings.extend(ppm_timings(&nexus_burst(&payload, 4)));
    let frames = decode(SubghzParams::default(), &keyed(&timings, RATE));
    assert_eq!(frames.len(), 1, "{frames:?}");
    assert_eq!(frames[0].repeats, 8, "both bursts must be counted");
    assert_eq!(
        frames[0].reading.as_ref().map(|r| r.temperature_c),
        Some(Some(21.3))
    );
}

fn ppm_burst(payload: &[u8], count: usize, short: u32, long: u32, sync: u32, repeats: u32) -> Ppm {
    Ppm {
        bits: payload_bits(payload, count),
        pulse_us: 500,
        short_gap_us: short,
        long_gap_us: long,
        sync_gap_us: sync,
        repeats,
    }
}

#[test]
fn reads_an_acurite_606_off_its_wider_pulse_position_burst() {
    let burst = ppm_burst(&[0x7B, 0x80, 0xBB, 0x76], 32, 2_000, 4_000, 9_000, 6);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Acurite-606TX");
    assert_eq!(reading.temperature_c, Some(18.7));
    assert_eq!(reading.channel, Some(1));
}

#[test]
fn reads_a_prologue_sensor_that_shares_its_timing_with_the_acurite() {
    let burst = ppm_burst(&[0x9C, 0x79, 0x0E, 0xA3, 0x70], 36, 2_000, 4_000, 9_000, 5);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Prologue-TH");
    assert_eq!(reading.temperature_c, Some(23.4));
    assert_eq!(reading.humidity_pct, Some(55.0));
}

#[test]
fn reads_an_infactory_sensor_through_the_tolerance_its_spec_asks_for() {
    let burst = ppm_burst(&[0x0F, 0x80, 0x65, 0x06, 0x23], 40, 2_000, 4_000, 16_000, 6);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "inFactory-TH");
    assert_eq!(reading.temperature_c, Some(22.0));
    assert_eq!(reading.humidity_pct, Some(62.0));
}

#[test]
fn reads_a_fine_offset_sensor_off_its_pulse_widths() {
    let bits = payload_bits(&[0xFF, 0x45, 0xA0, 0xC3, 0x49, 0xEB], 48);
    let mut timings = Vec::new();
    for &bit in &bits {
        timings.push(if bit { 544 } else { 1_524 });
        timings.push(1_036);
    }
    timings.pop();
    let frames = decode(SubghzParams::default(), &keyed(&timings, RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "FineOffset-WH2");
    assert_eq!(reading.temperature_c, Some(19.5));
    assert_eq!(reading.humidity_pct, Some(73.0));
}

#[test]
fn reads_a_wt450_off_its_differential_manchester_symbols() {
    let bits = payload_bits(&[0xC3, 0x42, 0xD4, 0x78, 0x50], 36);
    let mut timings = Vec::new();
    for &bit in &bits {
        if bit {
            timings.push(976);
            timings.push(976);
        } else {
            timings.push(1_952);
        }
    }
    let frames = decode(SubghzParams::default(), &keyed(&timings, RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "WT450-TH");
    assert_eq!(reading.temperature_c, Some(21.5));
    assert_eq!(reading.humidity_pct, Some(45.0));
}

#[test]
fn reads_an_f007th_off_a_manchester_carrier() {
    let mut bits = vec![true];
    bits.extend(payload_bits(&[0x14, 0x50], 12));
    bits.extend(payload_bits(&[0x45, 0x93, 0x24, 0x41, 0x30, 0x6F], 48));
    let frames = decode(SubghzParams::default(), &manchester(&bits, 500, 3, RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "AmbientWeather-F007TH");
    assert_eq!(reading.temperature_c, Some(20.5));
    assert_eq!(reading.humidity_pct, Some(48.0));
    assert_eq!(reading.channel, Some(3));
}

#[test]
fn reads_a_wh31e_off_a_frequency_shift_keyed_carrier() {
    let mut bits = [true, false].repeat(24);
    bits.extend(payload_bits(
        &[0x2D, 0xD4, 0x30, 0xC3, 0x82, 0x73, 0x33, 0xD0, 0xEB],
        72,
    ));
    let p = SubghzParams {
        modulation: SubghzModulation::Fsk,
        ..SubghzParams::default()
    };
    let frames = decode(p, &fsk_nrz(&bits, 56, 40_000.0, RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "AmbientWeather-WH31E");
    assert_eq!(reading.temperature_c, Some(22.7));
    assert_eq!(reading.humidity_pct, Some(51.0));
    assert_eq!(reading.channel, Some(1));
}

fn pwm_row(bits: &[bool], short_us: u32, long_us: u32) -> Vec<u32> {
    let mut timings = Vec::new();
    for &bit in bits {
        let (pulse, gap) = if bit {
            (short_us, long_us)
        } else {
            (long_us, short_us)
        };
        timings.push(pulse);
        timings.push(gap);
    }
    timings.pop();
    timings
}

#[test]
fn reads_a_rubicson_rather_than_the_nexus_it_shares_a_layout_with() {
    let burst = ppm_burst(&[0xB4, 0x80, 0xA8, 0xFA, 0xA0], 36, 1_000, 2_000, 4_000, 8);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Rubicson-Temperature");
    assert_eq!(reading.temperature_c, Some(16.8));
}

#[test]
fn reads_a_kedsum_past_the_two_lead_in_bits_of_its_row() {
    let mut bits = vec![false, false];
    bits.extend(payload_bits(&[0x3C, 0x90, 0x56, 0xE3, 0x02], 40));
    let mut timings = Vec::new();
    for _ in 0..5 {
        for &bit in &bits {
            timings.push(500);
            timings.push(if bit { 4_000 } else { 2_000 });
        }
        timings.push(500);
        timings.push(6_000);
    }
    timings.pop();
    let frames = decode(SubghzParams::default(), &keyed(&timings, RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Kedsum-TH");
    assert_eq!(reading.temperature_c, Some(22.0));
    assert_eq!(reading.humidity_pct, Some(62.0));
}

#[test]
fn reads_a_springfield_soil_probe() {
    let burst = ppm_burst(&[0x66, 0x10, 0xC2, 0x69, 0x00], 36, 2_000, 4_000, 6_000, 4);
    let frames = decode(SubghzParams::default(), &keyed(&ppm_timings(&burst), RATE));
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Springfield-Soil");
    assert_eq!(reading.moisture_pct, Some(60.0));
    assert_eq!(reading.temperature_c, Some(19.4));
}

#[test]
fn reads_an_auriol_whose_bits_arrive_the_other_way_up() {
    let bits: Vec<bool> = payload_bits(&[0x91, 0x26, 0x00, 0xF1, 0xB6], 40)
        .iter()
        .map(|&bit| !bit)
        .collect();
    let frames = decode(
        SubghzParams::default(),
        &keyed(&pwm_row(&bits, 252, 612), RATE),
    );
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Auriol-HG02832");
    assert_eq!(reading.temperature_c, Some(24.1));
    assert_eq!(reading.humidity_pct, Some(38.0));
}

#[test]
fn reads_a_wt0124_pool_thermometer() {
    let bits = payload_bits(&[0x55, 0xCA, 0x99, 0x20, 0x26, 0xFF, 0x00], 49);
    let frames = decode(
        SubghzParams::default(),
        &keyed(&pwm_row(&bits, 680, 1_850), RATE),
    );
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "WT0124-Pool");
    assert_eq!(reading.temperature_c, Some(26.5));
}

#[test]
fn reads_an_opus_soil_probe() {
    let bits = payload_bits(&[0xFF, 0x51, 0x2F, 0x3D, 0x00, 0xBD], 48);
    let frames = decode(
        SubghzParams::default(),
        &keyed(&pwm_row(&bits, 544, 932), RATE),
    );
    let reading = frames
        .iter()
        .find_map(|f| f.reading.as_ref())
        .expect("a sensor reading");
    assert_eq!(reading.model, "Opus-XT300");
    assert_eq!(reading.moisture_pct, Some(47.0));
    assert_eq!(reading.temperature_c, Some(21.0));
}

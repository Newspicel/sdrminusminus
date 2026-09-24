use xng_mode_vdl2::Vdl2ChannelDecoder;

use super::avlc::{AddressType, encode_address};
use super::modulate::{burst_iq, burst_iq_shaped};
use super::*;
use crate::testutil::{run_events, settings};
use crate::xng_adapter;

struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 as f32 / u64::MAX as f32) * 2.0 - 1.0
    }

    fn add(&mut self, iq: &mut [Complex<f32>], amplitude: f32) {
        for s in iq {
            *s += Complex::new(self.next() * amplitude, self.next() * amplitude);
        }
    }
}

fn aoa_frame() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(encode_address(
        AddressType::GroundIcao,
        0x10A234,
        false,
        false,
    ));
    f.extend(encode_address(AddressType::Aircraft, 0x800F5C, false, true));
    f.push(0x03);
    f.push(0xFF);
    f.extend(crate::acars::block::build(
        '2',
        "VT-ANB",
        None,
        "B6",
        '4',
        Some("M11A"),
        Some("AI0142"),
        "/BOMASAI.ADS.VT-ANB072501A070A988CA73248F0E5DC10200000F5EE1ABC000102B885E0A19F5",
        false,
    ));
    f
}

fn rr_frame() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(encode_address(AddressType::Aircraft, 0x800F5C, true, false));
    f.extend(encode_address(
        AddressType::GroundIcao,
        0x10A234,
        true,
        true,
    ));
    f.push(0x01);
    f
}

fn xid_frame() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(encode_address(
        AddressType::AllStations,
        0xFFFFFF,
        false,
        false,
    ));
    f.extend(encode_address(
        AddressType::GroundIcao,
        0x2C0A55,
        false,
        true,
    ));
    f.push(0xAF);
    let gs = encode_address(AddressType::GroundIcao, 0x2C0A55, false, true);
    let mut support = vec![0x0E, 0x71];
    support.extend_from_slice(&gs);
    let params_len = 2 + 1 + 2 + 4 + 2 + support.len();
    f.extend([0x82, 0xF0, 0x00, params_len as u8, 0x00, 0x01, b'V']);
    f.extend([0x83, 0x04, b'K', b'S', b'M', b'F']);
    f.extend([0xC0, support.len() as u8]);
    f.extend(support);
    f
}

fn cpdlc_frame() -> Vec<u8> {
    let apdu = super::cpdlc::build_downlink_wilco_for_test();
    let mut cotp = vec![0x04, 0xF0, 0x00, 0x01, 0x80];
    cotp.extend(apdu);
    let mut clnp = vec![0x81, 15, 1, 0x3F, 0x1C, 0x00, 0x00, 0x00, 0x00];
    clnp.extend([2, 0x47, 0x01, 2, 0x47, 0x02]);
    clnp.extend(cotp);
    let mut f = Vec::new();
    f.extend(encode_address(
        AddressType::GroundIcao,
        0x10A234,
        false,
        false,
    ));
    f.extend(encode_address(AddressType::Aircraft, 0x800F5C, false, true));
    f.push(0x00);
    f.extend([0x10, 0x23, 0x00]);
    f.extend(clnp);
    f
}

fn esis_frame() -> Vec<u8> {
    let mut f = Vec::new();
    f.extend(encode_address(
        AddressType::GroundIcao,
        0x10A234,
        false,
        false,
    ));
    f.extend(encode_address(AddressType::Aircraft, 0x800F5C, false, true));
    f.push(0x22);
    f.extend([
        0x82, 0x0E, 0x01, 0x00, 0x04, 0x02, 0x58, 0x00, 0x00, 3, 0x47, 0x00, 0x27,
    ]);
    f
}

fn decode_ours(rate: f64, iq: &[Complex<f32>]) -> Vec<Vdl2Frame> {
    let mut decoder = Vdl2Decoder::new(rate);
    let mut frames = Vec::new();
    for chunk in iq.chunks(1024) {
        decoder.process(chunk, &mut frames);
    }
    frames
}

fn padded(burst: Vec<Complex<f32>>, lead: usize) -> Vec<Complex<f32>> {
    let mut iq = vec![Complex::default(); lead];
    iq.extend(burst);
    iq.extend(vec![Complex::default(); 30_000]);
    iq
}

#[test]
fn decodes_burst_at_channel_rate() {
    let mut iq = padded(
        burst_iq(&[aoa_frame(), rr_frame()], 50_000.0, 0.0, 0.5),
        800,
    );
    Noise(0xabcd_ef01_2345_6789).add(&mut iq, 0.01);
    let frames = decode_ours(50_000.0, &iq);
    assert_eq!(frames.len(), 2);
    let acars = frames[0].acars.as_ref().expect("ACARS");
    assert!(acars.crc_ok);
    assert_eq!(acars.core.tail.as_deref(), Some("VT-ANB"));
    assert_eq!(acars.core.label, "B6");
    assert_eq!(acars.core.flight.as_deref(), Some("AI0142"));
    let app = acars.core.app.as_ref().expect("ADS-C decodes");
    assert_eq!(app["app"], "adsc");
    assert_eq!(frames[0].avlc.src.addr, "800F5C");
    assert_eq!(frames[0].avlc.dst.addr, "10A234");
    assert!(frames[1].acars.is_none());
}

#[test]
fn freq_skew_tracks_injected_cfo() {
    for cfo in [150.0_f64, -250.0, 400.0] {
        let iq = padded(burst_iq(&[aoa_frame()], 50_000.0, cfo, 0.5), 800);
        let frames = decode_ours(50_000.0, &iq);
        let skew = f64::from(frames.first().expect("decodes").freq_skew_hz);
        assert!((skew - cfo).abs() < 40.0, "skew {skew} vs {cfo}");
    }
}

#[test]
fn evm_snr_drops_with_more_noise() {
    let snr_at = |amp: f32| {
        let mut iq = padded(burst_iq(&[aoa_frame()], 50_000.0, 0.0, 0.5), 800);
        Noise(0x51b3_2c9f_a7e1_0d44).add(&mut iq, amp);
        decode_ours(50_000.0, &iq).first().map(|f| f.snr_db)
    };
    let clean = snr_at(0.01).expect("clean decodes");
    let noisy = snr_at(0.06).expect("noisy decodes");
    assert!(clean.is_finite() && noisy.is_finite());
    assert!(noisy < clean);
}

#[test]
fn decodes_pulse_shaped_burst() {
    for rate in [50_000.0, 100_000.0, 105_000.0] {
        let burst = burst_iq_shaped(&[aoa_frame(), rr_frame()], rate, 0.0, 0.5);
        let mut iq = padded(burst, (rate / 50.0) as usize);
        Noise(0x1357_9bdf_2468_ace0).add(&mut iq, 0.02);
        let frames = decode_ours(rate, &iq);
        assert_eq!(frames.len(), 2, "rate {rate}");
        assert!(frames[0].acars.is_some(), "rate {rate}");
    }
}

#[test]
fn decodes_atn_cpdlc_through_x25() {
    let iq = padded(burst_iq(&[cpdlc_frame()], RATE, 0.0, 0.5), 800);
    let frames = decode_ours(RATE, &iq);
    let atn = frames.first().and_then(|f| f.atn.as_ref()).expect("atn");
    assert_eq!(atn["layer"], "x25");
    assert_eq!(atn["network"]["cotp"]["app"]["application"], "CPDLC");
}

#[test]
fn decodes_an_avlc_supervisory_frame() {
    let iq = padded(burst_iq(&[rr_frame()], RATE, 0.0, 0.5), 800);
    let mut channel = Vdl2Channel::new(
        ChannelCtx { input_rate: RATE },
        settings(ChannelParams::Vdl2(Vdl2Params::default())),
    )
    .expect("channel");
    let events = run_events(&mut channel, &iq);
    let [DecoderEvent::Vdl2(message)] = &events[..] else {
        panic!("one message expected: {events:?}");
    };
    assert!(message.crc_ok);
    assert_eq!(message.message_type, "avlc-rr");
    assert_eq!(message.details["type"], "vdl2");
}

fn decode_xng(iq: &[Complex<f32>]) -> Vec<DataLinkMessage> {
    let mut decoder = Vdl2ChannelDecoder::new(RATE, 0.0).expect("xng decoder");
    let mut out = Vec::new();
    for chunk in iq.chunks(1024) {
        for frame in decoder.process(chunk) {
            out.push(xng_adapter::structured(xng_mode_vdl2::to_message(
                &frame,
                0,
                decoder.level_dbfs(),
                xng_adapter::provenance(),
            )));
        }
    }
    out
}

fn decode_channel(iq: &[Complex<f32>], decoder: Vdl2Decoder) -> Vec<DataLinkMessage> {
    let mut channel = Vdl2Channel {
        decoder,
        frames: Vec::new(),
    };
    let mut out = ChannelOutputs::default();
    let mut messages = Vec::new();
    for chunk in iq.chunks(1024) {
        out.reset();
        channel.process(chunk, &mut out);
        messages.extend(out.events.drain(..).filter_map(|event| match event {
            DecoderEvent::Vdl2(message) => Some(message),
            _ => None,
        }));
    }
    messages
}

fn equivalence_capture(seed: u64, noise: f32) -> Vec<Complex<f32>> {
    let bursts: [(Vec<Vec<u8>>, f64, bool); 6] = [
        (vec![aoa_frame(), rr_frame()], 0.0, true),
        (vec![xid_frame()], 220.0, false),
        (vec![cpdlc_frame()], -310.0, true),
        (vec![esis_frame(), rr_frame()], 90.0, true),
        (vec![aoa_frame()], -140.0, false),
        (vec![rr_frame()], 0.0, true),
    ];
    let mut iq = vec![Complex::default(); 3_000];
    for (frames, cfo, shaped) in bursts {
        let burst = if shaped {
            burst_iq_shaped(&frames, RATE, cfo, 0.4)
        } else {
            burst_iq(&frames, RATE, cfo, 0.4)
        };
        iq.extend(burst);
        iq.extend(vec![Complex::default(); 7_000]);
    }
    iq.extend(vec![Complex::default(); 40_000]);
    Noise(seed).add(&mut iq, noise);
    iq
}

fn assert_same(ours: &[DataLinkMessage], theirs: &[DataLinkMessage]) {
    assert_eq!(ours.len(), theirs.len());
    for (a, b) in ours.iter().zip(theirs) {
        assert_eq!(a.message_type, b.message_type);
        assert_eq!(a.station, b.station);
        assert_eq!(a.text, b.text);
        assert_eq!(a.crc_ok, b.crc_ok);
        assert_eq!(a.fec_corrected, b.fec_corrected);
        assert_eq!(a.raw, b.raw);
        assert_eq!(a.details, b.details);
        let close = |x: Option<f32>, y: Option<f32>| match (x, y) {
            (Some(x), Some(y)) => (x - y).abs() < 0.05,
            (x, y) => x == y,
        };
        assert!(close(a.snr_db, b.snr_db), "{:?} {:?}", a.snr_db, b.snr_db);
        assert!(close(a.frequency_error_hz, b.frequency_error_hz));
    }
}

const EQUIVALENCE_CASES: [(u64, f32); 5] = [
    (0x1111_2222_3333_4444, 0.01),
    (0x5555_6666_7777_8888, 0.05),
    (0x9999_aaaa_bbbb_cccc, 0.1),
    (0xdddd_eeee_ffff_0000, 0.14),
    (0x0123_4567_89ab_cdef, 0.18),
];

#[test]
fn differential_detection_matches_xng_exactly() {
    for (seed, noise) in EQUIVALENCE_CASES {
        let iq = equivalence_capture(seed, noise);
        let ours = decode_channel(&iq, Vdl2Decoder::differential(RATE));
        let theirs = decode_xng(&iq);
        assert!(
            noise > 0.15 || ours.len() == 8,
            "noise {noise}: {}",
            ours.len()
        );
        assert_same(&ours, &theirs);
    }
}

#[test]
fn default_detection_keeps_every_xng_frame() {
    let (mut ours_total, mut theirs_total) = (0, 0);
    for (seed, noise) in EQUIVALENCE_CASES {
        let iq = equivalence_capture(seed, noise);
        let ours = decode_channel(&iq, Vdl2Decoder::new(RATE));
        let theirs = decode_xng(&iq);
        for message in &theirs {
            let found = ours
                .iter()
                .find(|m| m.raw == message.raw)
                .expect("xng frame");
            assert_eq!(found.details, message.details);
            assert_eq!(found.message_type, message.message_type);
            assert_eq!(found.crc_ok, message.crc_ok);
        }
        ours_total += ours.len();
        theirs_total += theirs.len();
    }
    assert!(ours_total > theirs_total, "{ours_total} vs {theirs_total}");
}

fn sweep_capture(seed: u64, noise: f32, bursts: usize) -> Vec<Complex<f32>> {
    let mut rng = Noise(seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut iq = vec![Complex::default(); 3_000];
    for k in 0..bursts {
        let cfo = f64::from(rng.next()) * 500.0;
        let frames = if k % 2 == 0 {
            vec![aoa_frame(), rr_frame()]
        } else {
            vec![cpdlc_frame()]
        };
        iq.extend(burst_iq_shaped(&frames, RATE, cfo, 0.4));
        iq.extend(vec![
            Complex::default();
            4_000 + (rng.next().abs() * 3_000.0) as usize
        ]);
    }
    iq.extend(vec![Complex::default(); 40_000]);
    Noise(seed).add(&mut iq, noise);
    let mut filtered = Vec::new();
    channel_filter().process(&iq, &mut filtered);
    filtered
}

#[test]
#[ignore = "sensitivity sweep"]
fn sensitivity_sweep() {
    for noise in [0.14f32, 0.16, 0.18, 0.2, 0.22, 0.24] {
        let (mut ours, mut theirs) = (0, 0);
        for seed in 1..=6u64 {
            let iq = sweep_capture(seed * 0x1234_5678_9abc_def1, noise, 30);
            ours += decode_channel(&iq, Vdl2Decoder::new(RATE)).len();
            theirs += decode_xng(&iq).len();
        }
        eprintln!("SWEEP noise {noise}: ours {ours} xng {theirs}");
        assert!(ours >= theirs);
    }
}

#[test]
fn outdecodes_xng_near_the_noise_floor() {
    let iq = sweep_capture(0x2468_ace0_1357_9bdf, 0.2, 20);
    let ours = decode_channel(&iq, Vdl2Decoder::new(RATE)).len();
    let theirs = decode_xng(&iq).len();
    assert!(ours > 2 * theirs && ours > 25, "{ours} vs {theirs}");
}

#[test]
fn stays_silent_on_noise() {
    for (seed, noise) in [
        (0x0bad_cafe_f00d_d00d, 0.05f32),
        (0x1234_4321_abcd_dcba, 0.3),
    ] {
        let mut iq = vec![Complex::default(); 3_000_000];
        Noise(seed).add(&mut iq, noise);
        let mut filtered = Vec::new();
        channel_filter().process(&iq, &mut filtered);
        assert!(decode_ours(RATE, &filtered).is_empty(), "noise {noise}");
    }
}

use super::*;

fn access_units(mut bytes: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    while !bytes.is_empty() {
        let size = usize::from(u16::from_be_bytes([bytes[0], bytes[1]]));
        units.push(&bytes[2..2 + size]);
        bytes = &bytes[2 + size..];
    }
    units
}

#[test]
fn dab_lc_he_aac_and_parametric_stereo_decode_960_sample_access_units() {
    for (rate, sbr, ps, stereo, source, reference) in [
        (
            48000,
            false,
            false,
            false,
            include_bytes!("../../../../fixtures/broadcast_audio/dab_lc_mono_48k.aus").as_slice(),
            include_bytes!("../../../../fixtures/broadcast_audio/dab_lc_mono_48k.pcm").as_slice(),
        ),
        (
            48000,
            true,
            false,
            true,
            include_bytes!("../../../../fixtures/broadcast_audio/dab_he_stereo_48k.aus").as_slice(),
            include_bytes!("../../../../fixtures/broadcast_audio/dab_he_stereo_48k.pcm").as_slice(),
        ),
        (
            32000,
            true,
            true,
            false,
            include_bytes!("../../../../fixtures/broadcast_audio/dab_he_ps_32k.aus").as_slice(),
            include_bytes!("../../../../fixtures/broadcast_audio/dab_he_ps_32k_48k.pcm").as_slice(),
        ),
    ] {
        let format = AudioFormat {
            sample_rate_hz: rate,
            spectral_band_replication: sbr,
            parametric_stereo: ps,
            stereo_core: stereo,
            surround: 0,
        };
        let packets: Vec<u8> = access_units(source)
            .into_iter()
            .flat_map(|unit| latm::wrap(unit, format).expect("LATM"))
            .collect();
        let decoded = decode(Kind::Latm, &packets, 97);
        assert_eq!(decoded.len(), 30, "{format:?}");
        let pcm: Vec<f32> = decoded
            .into_iter()
            .flat_map(|(_, p)| match p {
                Payload::Audio(pcm) => pcm,
                _ => panic!("audio"),
            })
            .collect();
        let expected: Vec<f32> = reference
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert!(
            pcm.len().abs_diff(expected.len()) < 100,
            "{format:?}: {} != {}",
            pcm.len(),
            expected.len()
        );
        let best = (-128_i32..=128)
            .flat_map(|lag| [-1.0_f32, 1.0].map(move |sign| (lag, sign)))
            .map(|(lag, sign)| {
                let mut err = 0.0;
                let mut pow = 0.0;
                let gain = if stereo || ps {
                    1.0
                } else {
                    std::f32::consts::FRAC_1_SQRT_2
                };
                for i in 12000..pcm.len().min(expected.len()) - 4096 {
                    let actual = pcm[(i as i32 + 2 * lag) as usize];
                    let target = expected[i] * gain * sign;
                    err += f64::from(actual - target).powi(2);
                    pow += f64::from(target).powi(2);
                }
                (err / pow, lag, sign)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("alignment");

        assert!(
            best.0 < if sbr { 0.08 } else { 0.0001 },
            "{format:?}: relative error {best:?}"
        );
    }
}

fn decode(kind: Kind, data: &[u8], chunk: usize) -> Vec<(Option<i64>, Payload)> {
    let mut decoder = decoder::Decoder::new(kind).expect("decoder");
    let mut out = Vec::new();
    for block in data.chunks(chunk) {
        decoder.push(block, None, &mut out).expect("decode");
    }
    decoder.finish(&mut out).expect("flush");
    out
}

#[test]
fn dvb_aac_and_dolby_produce_stereo_pcm_across_pes_boundaries() {
    for (kind, data) in [
        (
            Kind::Aac,
            include_bytes!("../../../../fixtures/broadcast_audio/tone.aac").as_slice(),
        ),
        (
            Kind::Ac3,
            include_bytes!("../../../../fixtures/broadcast_audio/tone.ac3").as_slice(),
        ),
        (
            Kind::Eac3,
            include_bytes!("../../../../fixtures/broadcast_audio/tone.eac3").as_slice(),
        ),
    ] {
        let reference = decode(kind, data, data.len());
        let fragmented = decode(kind, data, 73);
        assert_eq!(reference.len(), fragmented.len());
        let mut energy = [0.0_f64; 2];
        let mut samples = 0;
        for ((_, a), (_, b)) in reference.iter().zip(&fragmented) {
            let (Payload::Audio(a), Payload::Audio(b)) = (a, b) else {
                panic!("audio");
            };
            assert_eq!(a, b);
            for pair in a.as_chunks::<2>().0 {
                energy[0] += f64::from(pair[0]).powi(2);
                energy[1] += f64::from(pair[1]).powi(2);
                samples += 1;
            }
        }
        assert!((18000..24000).contains(&samples), "{kind:?}: {samples}");
        assert!(
            energy[0] > 100.0 && energy[1] > 20.0,
            "{kind:?}: {energy:?}"
        );
        assert!(energy[0] > energy[1] * 3.0 && energy[0] < energy[1] * 5.0);
    }
}

#[test]
fn mpeg2_h264_and_hevc_decode_reordered_color_pictures_across_pes_boundaries() {
    for (kind, data) in [
        (
            Kind::Mpeg2,
            include_bytes!("../../../../fixtures/broadcast_audio/pattern.m2v").as_slice(),
        ),
        (
            Kind::H264,
            include_bytes!("../../../../fixtures/broadcast_audio/pattern.h264").as_slice(),
        ),
        (
            Kind::H265,
            include_bytes!("../../../../fixtures/broadcast_audio/pattern.hevc").as_slice(),
        ),
    ] {
        let reference = decode(kind, data, data.len());
        let fragmented = decode(kind, data, 173);
        assert_eq!(
            reference.len(),
            if kind == Kind::Mpeg2 { 12 } else { 10 },
            "{kind:?}"
        );
        assert_eq!(reference.len(), fragmented.len());
        for ((_, a), (_, b)) in reference.iter().zip(&fragmented) {
            let (Payload::Video(a), Payload::Video(b)) = (a, b) else {
                panic!("video");
            };
            assert_eq!(a, b);
            assert_eq!((a.width, a.height), (160, 96));
            assert_eq!(a.rgb.len(), 160 * 96 * 3);
            assert!(a.rgb.iter().any(|&v| v > 200));
            assert!(a.rgb.iter().any(|&v| v < 20));
        }
    }
}

#[test]
fn resetting_a_fragmented_parser_recovers_at_the_next_complete_stream() {
    let bytes = include_bytes!("../../../../fixtures/broadcast_audio/tone.aac");
    let mut decoder = decoder::Decoder::new(Kind::Aac).expect("decoder");
    let mut output = Vec::new();
    decoder
        .push(&bytes[..17], Some(0), &mut output)
        .expect("partial header");
    decoder.recover().expect("reset parser");
    decoder
        .push(bytes, Some(90000), &mut output)
        .expect("complete stream");
    decoder.finish(&mut output).expect("flush");
    let reference = decode(Kind::Aac, bytes, bytes.len());
    assert_eq!(output.len(), reference.len());
    for ((_, actual), (_, expected)) in output.iter().zip(&reference) {
        let (Payload::Audio(actual), Payload::Audio(expected)) = (actual, expected) else {
            panic!("audio")
        };
        assert_eq!(actual, expected);
    }
}

#[test]
fn data_input_errors_are_reported_as_data_errors() {
    let mut media = BroadcastMedia::new().expect("worker");
    media.push(Kind::DabPacket, &[0; CHUNK_BYTES], None, None);
    let mut out = ChannelOutputs::default();
    for _ in 0..100 {
        media.drain(&mut out);
        if media.data_errors > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        media.data_error.as_deref(),
        Some("DAB packet configuration missing")
    );
    media.push(
        Kind::DabPacket,
        &vec![0; CHUNK_BYTES * (INPUT_SLOTS + 1)],
        None,
        None,
    );
    assert_eq!(
        media.data_error.as_deref(),
        Some("Broadcast media input overflow")
    );
    assert_eq!(media.audio_errors, 0);
}

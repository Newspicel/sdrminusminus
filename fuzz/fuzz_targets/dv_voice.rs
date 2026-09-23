#![no_main]

use libfuzzer_sys::fuzz_target;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, ChannelRx, DpmrChannel,
    testgen::dv::dpmr::{self, Call},
};
use sdrmm_wire::{ChannelParams, ChannelSettings, DpmrParams, Squelch};

const VOICE_FRAME_BYTES: usize = 9;
const MAX_VOICE_FRAMES: usize = 32;

fuzz_target!(|data: &[u8]| {
    let Some((head, voice)) = data.split_first_chunk::<4>() else {
        return;
    };
    let voice: Vec<[bool; 72]> = voice
        .as_chunks::<VOICE_FRAME_BYTES>()
        .0
        .iter()
        .take(MAX_VOICE_FRAMES)
        .map(|frame| std::array::from_fn(|bit| frame[bit / 8] >> (bit % 8) & 1 == 1))
        .collect();
    if voice.is_empty() {
        return;
    }
    let call = Call {
        colour_code: u16::from(head[0]) << 8 | u16::from(head[1]),
        called: u32::from(head[2]),
        own: u32::from(head[3]),
        mode: head[0] & 3,
    };
    let rate = DpmrChannel::descriptor().input_rate_hz;
    let settings = ChannelSettings {
        frequency_hz: 0.0,
        squelch: Squelch::Off,
        params: ChannelParams::Dpmr(DpmrParams::default()),
        blanker: Default::default(),
    };
    let Ok(mut channel) = DpmrChannel::new(ChannelCtx { input_rate: rate }, settings) else {
        return;
    };
    let iq = dpmr::transmission_with_voice(&call, &voice, rate);
    let mut out = ChannelOutputs::default();
    for block in iq.chunks(1_024) {
        out.reset();
        channel.process(block, &mut out);
        assert!(
            out.audio_pcm.iter().all(|sample| sample.is_finite()),
            "the dPMR vocoder produced a non-finite sample"
        );
        assert!(
            out.audio_pcm.iter().all(|sample| sample.abs() <= 1.0),
            "the dPMR vocoder produced a sample outside full scale"
        );
    }
});

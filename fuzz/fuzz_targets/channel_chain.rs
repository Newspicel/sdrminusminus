#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_wire::{ChannelDescriptor, ChannelSettings};

static DESCRIPTORS: OnceLock<Vec<ChannelDescriptor>> = OnceLock::new();

fn sample(byte: u8) -> f32 {
    (f32::from(byte) - 127.5) / 127.5
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, iq)) = data.split_first() else {
        return;
    };
    let descriptors = DESCRIPTORS.get_or_init(sdrmm_channels::descriptors);
    let descriptor = &descriptors[usize::from(selector) % descriptors.len()];
    let Some(settings) = ChannelSettings::default_for(&descriptor.type_id) else {
        return;
    };
    let ctx = ChannelCtx {
        input_rate: descriptor.input_rate_hz,
    };
    let Ok(mut channel) = sdrmm_channels::create(ctx, &settings) else {
        return;
    };
    let mut out = ChannelOutputs::default();
    let iq: Vec<Complex<f32>> = iq
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Complex::new(sample(pair[0]), sample(pair[1])))
        .collect();
    for block in iq.chunks(256) {
        out.reset();
        channel.process(block, &mut out);
        assert!(
            out.audio_pcm.iter().all(|sample| sample.is_finite()),
            "{} produced a non-finite sample",
            descriptor.type_id
        );
        assert!(
            out.audio_pcm.iter().all(|sample| sample.abs() <= 1.0),
            "{} produced a sample outside full scale",
            descriptor.type_id
        );
    }
});

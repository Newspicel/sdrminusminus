#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use num_complex::Complex;
use sdrmm_channels::{ChannelCtx, ChannelOutputs};
use sdrmm_wire::{ChannelDescriptor, ChannelSettings};

static DESCRIPTORS: OnceLock<Vec<ChannelDescriptor>> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let Ok(settings) = serde_json::from_slice::<ChannelSettings>(data) else {
        return;
    };
    let descriptors = DESCRIPTORS.get_or_init(sdrmm_channels::descriptors);
    let Some(descriptor) = descriptors
        .iter()
        .find(|descriptor| descriptor.type_id == settings.params.type_id())
    else {
        return;
    };
    let ctx = ChannelCtx {
        input_rate: descriptor.input_rate_hz,
    };
    let Ok(mut channel) = sdrmm_channels::create(ctx, &settings) else {
        return;
    };
    let _ = sdrmm_channels::channel_filter(&settings.params);
    let _ = sdrmm_channels::occupied_band(&settings.params);
    let mut out = ChannelOutputs::default();
    channel.process(&[Complex::new(0.0, 0.0); 512], &mut out);
    assert!(
        out.audio_pcm.iter().all(|sample| sample.is_finite()),
        "{} produced a non-finite sample from silence",
        descriptor.type_id
    );
});

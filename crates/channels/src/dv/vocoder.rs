//! Vocoder adapters shared by the digital-voice air interfaces.
use std::{
    alloc::{Layout, handle_alloc_error},
    ffi::c_void,
    ptr::NonNull,
};

use blip25_vocoder::{
    fullrate,
    halfrate::frame::decode_code_vectors,
    vocoder::{FrameStatus, Rate, Vocoder},
};
use codec2::{Codec2, Codec2Mode};
use num_complex::Complex;
use sdrmm_dsp::FracResampler;

use crate::{AUDIO_RATE, ChannelOutputs, clamp_full_scale};

const VOCODER_RATE_HZ: f64 = 8_000.0;
const MBE_FRAME_SAMPLES: usize = 160;
const OUTPUT_SAMPLES_PER_MBE_FRAME: usize = 960;

/// AMBE+2 3,600 x 2,450 code-vector interleave shared by DMR, NXDN EHR, dPMR and YSF V/D1.
/// Rows are codec vectors c0..c3 and columns are LSB-based bit positions.
pub(crate) const AMBE_3600_INTERLEAVE: [[(u8, u8); 2]; 36] = [
    [(0, 23), (0, 5)],
    [(1, 10), (2, 3)],
    [(0, 22), (0, 4)],
    [(1, 9), (2, 2)],
    [(0, 21), (0, 3)],
    [(1, 8), (2, 1)],
    [(0, 20), (0, 2)],
    [(1, 7), (2, 0)],
    [(0, 19), (0, 1)],
    [(1, 6), (3, 13)],
    [(0, 18), (0, 0)],
    [(1, 5), (3, 12)],
    [(0, 17), (1, 22)],
    [(1, 4), (3, 11)],
    [(0, 16), (1, 21)],
    [(1, 3), (3, 10)],
    [(0, 15), (1, 20)],
    [(1, 2), (3, 9)],
    [(0, 14), (1, 19)],
    [(1, 1), (3, 8)],
    [(0, 13), (1, 18)],
    [(1, 0), (3, 7)],
    [(0, 12), (1, 17)],
    [(2, 10), (3, 6)],
    [(0, 11), (1, 16)],
    [(2, 9), (3, 5)],
    [(0, 10), (1, 15)],
    [(2, 8), (3, 4)],
    [(0, 9), (1, 14)],
    [(2, 7), (3, 3)],
    [(0, 8), (1, 13)],
    [(2, 6), (3, 2)],
    [(0, 7), (1, 12)],
    [(2, 5), (3, 1)],
    [(0, 6), (1, 11)],
    [(2, 4), (3, 0)],
];

/// One stateful 8 kHz -> application-rate output path. Vocoders are predictive, and so is the
/// fractional resampler; both are deliberately kept per channel rather than reconstructed per
/// RF frame.
struct PcmOutput {
    resampler: FracResampler,
    pcm_8k: Vec<Complex<f32>>,
    pcm_48k: Vec<Complex<f32>>,
}

impl PcmOutput {
    fn new() -> Self {
        Self {
            resampler: FracResampler::new(f64::from(AUDIO_RATE) / VOCODER_RATE_HZ),
            pcm_8k: Vec::with_capacity(320),
            pcm_48k: Vec::with_capacity(1_920),
        }
    }

    fn reset(&mut self) {
        self.resampler = FracResampler::new(f64::from(AUDIO_RATE) / VOCODER_RATE_HZ);
        self.pcm_8k.clear();
        self.pcm_48k.clear();
    }

    fn append_i16(&mut self, pcm: &[i16], out: &mut ChannelOutputs) {
        self.pcm_8k.clear();
        self.pcm_8k.extend(
            pcm.iter()
                .map(|&sample| Complex::new(f32::from(sample) / 32_768.0, 0.0)),
        );
        self.append(out);
    }

    fn append_f32(&mut self, pcm: &[f32], out: &mut ChannelOutputs) {
        self.pcm_8k.clear();
        self.pcm_8k.extend(
            pcm.iter()
                .map(|&sample| Complex::new(sample / 32_768.0, 0.0)),
        );
        self.append(out);
    }

    fn append(&mut self, out: &mut ChannelOutputs) {
        self.pcm_48k.clear();
        self.resampler.process(&self.pcm_8k, &mut self.pcm_48k);
        out.audio_pcm
            .extend(self.pcm_48k.iter().map(|sample| sample.re));
        clamp_full_scale(&mut out.audio_pcm);
        if !self.pcm_48k.is_empty() {
            out.audio_rate = AUDIO_RATE;
        }
    }

    fn silence(frames: usize, out: &mut ChannelOutputs) {
        out.audio_pcm.extend(std::iter::repeat_n(
            0.0,
            frames * OUTPUT_SAMPLES_PER_MBE_FRAME,
        ));
        out.audio_rate = AUDIO_RATE;
    }
}

pub(crate) struct MbeDecoder {
    vocoder: Vocoder,
    output: PcmOutput,
}

impl MbeDecoder {
    pub(crate) fn half_rate() -> Self {
        Self::new(Rate::HalfRate3600x2450)
    }

    pub(crate) fn full_rate() -> Self {
        Self::new(Rate::FullRate7200x4400)
    }

    fn new(rate: Rate) -> Self {
        Self {
            vocoder: Vocoder::new(rate),
            output: PcmOutput::new(),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.vocoder.reset();
        self.output.reset();
    }

    /// Decode a carrier-specific AMBE+2 interleave after it has been assembled into the four
    /// codec code vectors.
    pub(crate) fn decode_half_code_vectors(
        &mut self,
        code: [u32; 4],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        if encrypted {
            PcmOutput::silence(1, out);
            return;
        }
        let frame = decode_code_vectors(code);
        let status = if frame.errors[0] == u8::MAX {
            FrameStatus::LOST
        } else {
            FrameStatus::new(u32::from(frame.error_total()), false)
        };
        if let Ok(pcm) = self.vocoder.decode_info(&frame.info, status) {
            self.output.append_i16(&pcm, out);
        }
    }

    /// Decode the natural 49-bit AMBE+2 information order used by YSF V/D mode 2.
    pub(crate) fn decode_half_info(
        &mut self,
        bits: &[bool; 49],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        if encrypted {
            PcmOutput::silence(1, out);
            return;
        }
        let mut packed = [0u8; 7];
        for (i, &bit) in bits.iter().enumerate() {
            packed[i / 8] |= u8::from(bit) << (7 - i % 8);
        }
        let info = blip25_vocoder::halfrate::unpack_natural(&packed);
        if let Ok(pcm) = self.vocoder.decode_info(&info, FrameStatus::new(0, false)) {
            self.output.append_i16(&pcm, out);
        }
    }

    /// Decode a P25 Annex-H full-rate frame in air-interface dibit order.
    pub(crate) fn decode_full_dibits(
        &mut self,
        dibits: &[u8; 72],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        if encrypted {
            PcmOutput::silence(1, out);
            return;
        }
        let frame = fullrate::frame::decode_frame(dibits);
        let status = FrameStatus::new(u32::from(frame.error_total()), false);
        if let Ok(pcm) = self.vocoder.decode_info(&frame.info, status) {
            self.output.append_i16(&pcm, out);
        }
    }

    /// Decode full-rate code vectors supplied by a non-P25 carrier such as YSF Voice FR.
    pub(crate) fn decode_full_code_vectors(
        &mut self,
        code: [u32; 8],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        let dibits = fullrate::fec::interleave(&code);
        self.decode_full_dibits(&dibits, encrypted, out);
    }
}

unsafe extern "C" {
    fn sdrmm_dstar_vocoder_new() -> *mut c_void;
    fn sdrmm_dstar_vocoder_free(decoder: *mut c_void);
    fn sdrmm_dstar_vocoder_reset(decoder: *mut c_void);
    fn sdrmm_dstar_vocoder_decode(decoder: *mut c_void, bits: *const u8, pcm: *mut f32) -> i32;
}

/// D-STAR's first-generation 3,600 x 2,400 AMBE decoder. The native state is owned uniquely
/// and is only touched through `&mut self`; moving a channel between DSP threads is safe.
pub(crate) struct DstarVocoder {
    decoder: NonNull<c_void>,
    output: PcmOutput,
    bits: [u8; 72],
    pcm: [f32; MBE_FRAME_SAMPLES],
}

// SAFETY: the opaque allocation has no thread affinity or shared global state, and every FFI
// call requires exclusive access to this owner.
unsafe impl Send for DstarVocoder {}

impl DstarVocoder {
    pub(crate) fn new() -> Self {
        // SAFETY: constructor takes no pointers and returns either a unique allocation or null.
        let decoder = unsafe { sdrmm_dstar_vocoder_new() };
        let Some(decoder) = NonNull::new(decoder) else {
            handle_alloc_error(Layout::new::<DstarVocoder>());
        };
        Self {
            decoder,
            output: PcmOutput::new(),
            bits: [0; 72],
            pcm: [0.0; MBE_FRAME_SAMPLES],
        }
    }

    pub(crate) fn reset(&mut self) {
        // SAFETY: `decoder` remains a live, uniquely owned allocation until `drop`.
        unsafe { sdrmm_dstar_vocoder_reset(self.decoder.as_ptr()) };
        self.output.reset();
    }

    pub(crate) fn decode(&mut self, bits: &[bool; 72], encrypted: bool, out: &mut ChannelOutputs) {
        if encrypted {
            PcmOutput::silence(1, out);
            return;
        }
        for (slot, &bit) in self.bits.iter_mut().zip(bits) {
            *slot = u8::from(bit);
        }
        // SAFETY: all pointers reference fixed-size live buffers of the lengths required by
        // the wrapper, and the decoder is uniquely borrowed for the call.
        unsafe {
            sdrmm_dstar_vocoder_decode(
                self.decoder.as_ptr(),
                self.bits.as_ptr(),
                self.pcm.as_mut_ptr(),
            )
        };
        self.output.append_f32(&self.pcm, out);
    }
}

impl Drop for DstarVocoder {
    fn drop(&mut self) {
        // SAFETY: this is the one matching free for the allocation and runs once.
        unsafe { sdrmm_dstar_vocoder_free(self.decoder.as_ptr()) };
    }
}

pub(crate) struct Codec2Decoder {
    codec_3200: Codec2,
    codec_1600: Codec2,
    output: PcmOutput,
    pcm: [i16; 320],
}

impl Codec2Decoder {
    pub(crate) fn new() -> Self {
        Self {
            codec_3200: Codec2::new(Codec2Mode::MODE_3200),
            codec_1600: Codec2::new(Codec2Mode::MODE_1600),
            output: PcmOutput::new(),
            pcm: [0; 320],
        }
    }

    pub(crate) fn reset(&mut self) {
        self.codec_3200 = Codec2::new(Codec2Mode::MODE_3200);
        self.codec_1600 = Codec2::new(Codec2Mode::MODE_1600);
        self.output.reset();
    }

    /// A voice-only M17 stream carries two 64-bit Codec2 3200 frames per radio frame.
    pub(crate) fn decode_3200(
        &mut self,
        payload: &[u8; 16],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        if encrypted {
            PcmOutput::silence(2, out);
            return;
        }
        for frame in payload.as_chunks::<8>().0 {
            self.codec_3200.decode(&mut self.pcm[..160], frame);
            self.output.append_i16(&self.pcm[..160], out);
        }
    }

    /// Voice+data M17 uses one 64-bit Codec2 1600 frame (40 ms) followed by 64 data bits.
    pub(crate) fn decode_1600(
        &mut self,
        payload: &[u8; 16],
        encrypted: bool,
        out: &mut ChannelOutputs,
    ) {
        if encrypted {
            PcmOutput::silence(2, out);
            return;
        }
        self.codec_1600.decode(&mut self.pcm, &payload[..8]);
        self.output.append_i16(&self.pcm, out);
    }
}

/// Assemble one 72-bit carrier interleave into AMBE+2's four code vectors. Each table row is
/// the `(vector, LSB-based bit)` destination for the high and low bit of one air dibit.
pub(crate) fn half_rate_code_vectors(
    frame: &[bool; 72],
    interleave: &[[(u8, u8); 2]; 36],
) -> [u32; 4] {
    let mut code = [0u32; 4];
    for (dibit, places) in frame.as_chunks::<2>().0.iter().zip(interleave) {
        for (&bit, &(row, column)) in dibit.iter().zip(places) {
            code[usize::from(row)] |= u32::from(bit) << column;
        }
    }
    code
}

#[cfg(test)]
pub(crate) mod testutil {
    use blip25_vocoder::{
        halfrate::frame::encode_code_vectors,
        vocoder::{Rate, Vocoder},
    };

    use super::AMBE_3600_INTERLEAVE;

    fn tone(frame: usize) -> [i16; 160] {
        std::array::from_fn(|i| {
            let sample = frame * 160 + i;
            (12_000.0 * (std::f64::consts::TAU * 440.0 * sample as f64 / 8_000.0).sin()) as i16
        })
    }

    /// AMBE+2 frames in the carrier interleave shared by NXDN, dPMR and YSF V/D1.
    pub(crate) fn half_rate_frames(count: usize) -> Vec<[bool; 72]> {
        let mut encoder = Vocoder::new(Rate::HalfRate3600x2450);
        (0..count)
            .map(|frame| {
                let info: [u16; 4] = encoder
                    .encode_info(&tone(frame))
                    .expect("encode half-rate tone")
                    .try_into()
                    .expect("four half-rate information vectors");
                let code = encode_code_vectors(&info);
                let mut air = [false; 72];
                for (dibit, places) in air
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(AMBE_3600_INTERLEAVE)
                {
                    for (bit, (row, column)) in dibit.iter_mut().zip(places) {
                        *bit = code[usize::from(row)] >> column & 1 == 1;
                    }
                }
                air
            })
            .collect()
    }

    /// Natural 49-bit AMBE+2 information frames for YSF V/D2.
    pub(crate) fn natural_half_rate_frames(count: usize) -> Vec<[bool; 49]> {
        let mut encoder = Vocoder::new(Rate::HalfRate2450x2450);
        (0..count)
            .map(|frame| {
                let bytes = encoder
                    .encode_pcm(&tone(frame))
                    .expect("encode natural half-rate tone");
                std::array::from_fn(|bit| bytes[bit / 8] >> (7 - bit % 8) & 1 == 1)
            })
            .collect()
    }

    /// Annex-H full-rate IMBE frames for P25. The YSF generator converts these to Voice-FR.
    pub(crate) fn full_rate_frames(count: usize) -> Vec<[bool; 144]> {
        let mut encoder = Vocoder::new(Rate::FullRate7200x4400);
        (0..count)
            .map(|frame| {
                let bytes = encoder
                    .encode_pcm(&tone(frame))
                    .expect("encode full-rate tone");
                std::array::from_fn(|bit| bytes[bit / 8] >> (7 - bit % 8) & 1 == 1)
            })
            .collect()
    }
}

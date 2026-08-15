//! FreeDV 1600: the interoperable FDMDV waveform and Codec2 1300 speech.

use std::{ffi::c_void, ptr::NonNull, sync::LazyLock};

use num_complex::Complex;
use sdrmm_dsp::{FirC, design_lowpass, golay23_correct};
use sdrmm_wire::{
    ChannelDescriptor, ChannelParams, ChannelSettings, DecoderEvent, DvFrame, DvFrameKind, DvMode,
    FreeDvMode, FreeDvParams, Sideband,
};

use super::vocoder::Codec2Decoder;
use crate::{ChannelCtx, ChannelError, ChannelFilter, ChannelOutputs, ChannelRx, check_input_rate};

const INPUT_RATE_HZ: f64 = 8_000.0;
const LOW_EDGE_HZ: f64 = 800.0;
const HIGH_EDGE_HZ: f64 = 2_200.0;
const FILTER_TAPS: usize = 257;
const MODEM_SCALE: f32 = 32_768.0 / 825.0;
const MODEM_BITS: usize = 32;
const MAX_MODEM_SAMPLES: usize = 200;
const CODEC_BITS: usize = 52;

static DESCRIPTOR: LazyLock<ChannelDescriptor> = LazyLock::new(|| ChannelDescriptor {
    type_id: "freedv".to_owned(),
    name: "FreeDV 1600".to_owned(),
    bandwidth_hz: HIGH_EDGE_HZ - LOW_EDGE_HZ,
    input_rate_hz: INPUT_RATE_HZ,
    has_audio: true,
    decoder_kind: Some("dv".to_owned()),
    ..ChannelDescriptor::default()
});

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FfiComplex {
    re: f32,
    im: f32,
}

#[repr(C)]
struct FdmdvResult {
    next_nin: i32,
    reliable_sync: i32,
    sync: i32,
}

unsafe extern "C" {
    fn sdrmm_fdmdv_create() -> *mut c_void;
    fn sdrmm_fdmdv_destroy(modem: *mut c_void);
    fn sdrmm_fdmdv_demod(
        modem: *mut c_void,
        input: *const FfiComplex,
        nin: i32,
        output: *mut u8,
    ) -> FdmdvResult;
}

struct Fdmdv {
    state: NonNull<c_void>,
    next_nin: usize,
}

// SAFETY: the opaque modem has no shared global state, is owned by one channel, and every FFI
// call requires exclusive access to this owner.
unsafe impl Send for Fdmdv {}

impl Fdmdv {
    fn new() -> Result<Self, ChannelError> {
        // SAFETY: the constructor takes no borrowed state and returns one independently owned
        // modem, freed by this type's Drop implementation.
        let state = unsafe { sdrmm_fdmdv_create() };
        Ok(Self {
            state: NonNull::new(state).ok_or_else(|| {
                ChannelError::InvalidSettings("FreeDV modem allocation failed".to_owned())
            })?,
            next_nin: 160,
        })
    }

    fn demod(&mut self, input: &[FfiComplex], bits: &mut [u8; MODEM_BITS]) -> FdmdvResult {
        debug_assert_eq!(input.len(), self.next_nin);
        // SAFETY: state is live until Drop, `input` contains exactly `next_nin` elements, and
        // the fixed output has the 32 bytes the wrapper writes.
        let result = unsafe {
            sdrmm_fdmdv_demod(
                self.state.as_ptr(),
                input.as_ptr(),
                input.len() as i32,
                bits.as_mut_ptr(),
            )
        };
        self.next_nin = usize::try_from(result.next_nin)
            .ok()
            .filter(|&nin| (1..=MAX_MODEM_SAMPLES).contains(&nin))
            .unwrap_or(160);
        result
    }
}

impl Drop for Fdmdv {
    fn drop(&mut self) {
        // SAFETY: this is the one matching destroy for the live pointer and runs once.
        unsafe { sdrmm_fdmdv_destroy(self.state.as_ptr()) };
    }
}

pub struct FreeDvChannel {
    sideband: Sideband,
    modem: Fdmdv,
    modem_input: [FfiComplex; MAX_MODEM_SAMPLES],
    modem_filled: usize,
    bits: [u8; MODEM_BITS],
    paired_bits: [u8; MODEM_BITS * 2],
    even_frame: bool,
    synced: bool,
    vocoder: Codec2Decoder,
}

fn params(settings: &ChannelSettings) -> Result<&FreeDvParams, ChannelError> {
    match &settings.params {
        ChannelParams::Freedv(params) if params.mode == FreeDvMode::Mode1600 => Ok(params),
        other => Err(ChannelError::InvalidSettings(format!(
            "freedv channel got {} params",
            other.type_id()
        ))),
    }
}

pub(crate) fn occupied_band(params: &FreeDvParams) -> (f64, f64) {
    match params.sideband {
        Sideband::Usb => (LOW_EDGE_HZ, HIGH_EDGE_HZ),
        Sideband::Lsb => (-HIGH_EDGE_HZ, -LOW_EDGE_HZ),
    }
}

pub(crate) fn channel_filter(params: &FreeDvParams) -> Result<ChannelFilter, ChannelError> {
    let (low, high) = occupied_band(params);
    let half_width = (high - low) / 2.0;
    let prototype = design_lowpass(FILTER_TAPS, half_width / INPUT_RATE_HZ);
    Ok(ChannelFilter::Sideband(FirC::from_lowpass(
        &prototype,
        (high + low) / 2.0 / INPUT_RATE_HZ,
    )))
}

impl ChannelRx for FreeDvChannel {
    fn descriptor() -> &'static ChannelDescriptor {
        &DESCRIPTOR
    }

    fn new(ctx: ChannelCtx, settings: ChannelSettings) -> Result<Self, ChannelError> {
        check_input_rate(ctx, &DESCRIPTOR)?;
        let params = params(&settings)?;
        Ok(Self {
            sideband: params.sideband,
            modem: Fdmdv::new()?,
            modem_input: [FfiComplex::default(); MAX_MODEM_SAMPLES],
            modem_filled: 0,
            bits: [0; MODEM_BITS],
            paired_bits: [0; MODEM_BITS * 2],
            even_frame: false,
            synced: false,
            vocoder: Codec2Decoder::new(),
        })
    }

    fn apply(&mut self, settings: ChannelSettings) -> Result<(), ChannelError> {
        let sideband = params(&settings)?.sideband;
        if sideband != self.sideband {
            self.modem = Fdmdv::new()?;
            self.sideband = sideband;
            self.reset_stream_state();
        }
        Ok(())
    }

    fn retuned(&mut self) {
        if let Ok(modem) = Fdmdv::new() {
            self.modem = modem;
        }
        self.reset_stream_state();
    }

    fn process(&mut self, iq: &[Complex<f32>], out: &mut ChannelOutputs) {
        for &sample in iq {
            let sample = match self.sideband {
                Sideband::Usb => sample,
                Sideband::Lsb => sample.conj(),
            } * MODEM_SCALE;
            self.modem_input[self.modem_filled] = FfiComplex {
                re: sample.re,
                im: sample.im,
            };
            self.modem_filled += 1;
            if self.modem_filled == self.modem.next_nin {
                self.demod_frame(out);
                self.modem_filled = 0;
            }
        }
    }
}

impl FreeDvChannel {
    fn reset_stream_state(&mut self) {
        self.modem_filled = 0;
        self.even_frame = false;
        self.synced = false;
        self.vocoder.reset();
    }
    fn demod_frame(&mut self, out: &mut ChannelOutputs) {
        let result = self
            .modem
            .demod(&self.modem_input[..self.modem_filled], &mut self.bits);
        let sync = result.sync != 0;
        if sync && !self.synced {
            let mut frame = DvFrame::new(DvMode::FreeDv, DvFrameKind::Header);
            frame.opcode = Some("1600".to_owned());
            out.events.push(DecoderEvent::Dv(frame));
        } else if !sync && self.synced {
            out.events.push(DecoderEvent::Dv(DvFrame::new(
                DvMode::FreeDv,
                DvFrameKind::Terminator,
            )));
        }
        self.synced = sync;

        if result.reliable_sync != 0 {
            self.even_frame = true;
        }
        if !sync {
            return;
        }
        let offset = if self.even_frame { MODEM_BITS } else { 0 };
        self.paired_bits[offset..offset + MODEM_BITS].copy_from_slice(&self.bits);
        if self.even_frame {
            self.decode_voice(out);
        }
        self.even_frame = !self.even_frame;
    }

    fn decode_voice(&mut self, out: &mut ChannelOutputs) {
        let mut received = 0u32;
        for index in 0..8 {
            received = received << 1 | u32::from(self.paired_bits[index]);
        }
        for index in 11..15 {
            received = received << 1 | u32::from(self.paired_bits[index]);
        }
        for index in CODEC_BITS..CODEC_BITS + 11 {
            received = received << 1 | u32::from(self.paired_bits[index]);
        }
        let Some((corrected, _errors)) = golay23_correct(received) else {
            return;
        };

        let mut payload = [0u8; CODEC_BITS];
        payload.copy_from_slice(&self.paired_bits[..CODEC_BITS]);
        for (index, bit) in payload[..8].iter_mut().enumerate() {
            *bit = ((corrected >> (22 - index)) & 1) as u8;
        }
        for (index, bit) in payload[11..15].iter_mut().enumerate() {
            *bit = ((corrected >> (14 - index)) & 1) as u8;
        }
        payload[2] = u8::from(payload[1] != 0 || payload[3] != 0);

        let mut packed = [0u8; 7];
        for (index, &bit) in payload.iter().enumerate() {
            packed[index / 8] |= bit << (7 - index % 8);
        }
        self.vocoder.decode_1300(&packed, out);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testutil::settings;

    #[test]
    fn free_dv_uses_the_selected_sideband() {
        assert_eq!(
            occupied_band(&FreeDvParams::default()),
            (LOW_EDGE_HZ, HIGH_EDGE_HZ)
        );
        assert_eq!(
            occupied_band(&FreeDvParams {
                sideband: Sideband::Lsb,
                ..FreeDvParams::default()
            }),
            (-HIGH_EDGE_HZ, -LOW_EDGE_HZ)
        );
    }

    #[test]
    fn payload_rebuild_matches_the_free_dv_golay_layout() {
        let data = 0xA53u16;
        let codeword = sdrmm_dsp::golay23_encode(data);
        for damaged in [codeword, codeword ^ 1 << 7, codeword ^ 1 << 1 ^ 1 << 19] {
            let (corrected, errors) = golay23_correct(damaged).unwrap();
            assert_eq!(corrected >> 11, u32::from(data));
            assert_eq!(errors, (damaged ^ codeword).count_ones());
        }
    }

    #[test]
    fn decodes_the_upstream_receive_recording() {
        const FIXTURE: &[u8] = include_bytes!("../../../../fixtures/freedv_1600_8k.sigmf-data");
        let iq: Vec<Complex<f32>> = FIXTURE
            .as_chunks::<8>()
            .0
            .iter()
            .map(|sample| {
                Complex::new(
                    f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
                    f32::from_le_bytes([sample[4], sample[5], sample[6], sample[7]]),
                )
            })
            .collect();
        for sideband in [Sideband::Usb, Sideband::Lsb] {
            let params = FreeDvParams {
                sideband,
                ..FreeDvParams::default()
            };
            let mut channel = FreeDvChannel::new(
                ChannelCtx {
                    input_rate: INPUT_RATE_HZ,
                },
                settings(ChannelParams::Freedv(params)),
            )
            .unwrap();
            let mut filter = channel_filter(&params).unwrap();
            let mut filtered = Vec::new();
            let mut out = ChannelOutputs::default();
            let mut frames = Vec::new();
            let mut audio = Vec::new();
            let started = Instant::now();
            for block in iq.chunks(997) {
                filter.process(block, &mut filtered);
                out.reset();
                channel.process(&filtered, &mut out);
                frames.append(&mut out.events);
                audio.extend_from_slice(&out.audio_pcm);
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "three seconds of FreeDV must decode faster than real time"
            );
            assert!(
                frames.iter().any(|event| matches!(
                    event,
                    DecoderEvent::Dv(frame)
                        if frame.mode == DvMode::FreeDv && frame.kind == DvFrameKind::Header
                )),
                "FreeDV modem never acquired {sideband:?} sync: {frames:?}"
            );
            assert!(
                audio.iter().any(|sample| sample.abs() > 0.001),
                "Codec2 produced no {sideband:?} speech audio"
            );
        }
    }
}

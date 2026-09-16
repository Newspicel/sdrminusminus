use std::ptr::NonNull;

use ffmpeg_the_third::{
    self as av, codec, ffi, frame, software,
    util::format::{Pixel, Sample, sample::Type},
};

use super::{Kind, Payload};
use crate::{AUDIO_RATE, VideoPicture};

pub struct Decoder {
    kind: Kind,
    next_pts: Option<i64>,
    pending_bytes: usize,
    parser: NonNull<ffi::AVCodecParserContext>,
    codec: codec::decoder::Opened,
    resampler: Option<software::resampling::Context>,
    scaler: Option<software::scaling::Context>,
    audio_spec: Option<(Sample, u32, Option<av::ChannelLayoutMask>)>,
    video_spec: Option<(Pixel, u32, u32, u32)>,
}

// A decoder handed noise complains to stderr about every frame it cannot make sense of, which a
// receiver tuned away from a transmitter does constantly. Every one of those is already reported
// through the channel's own error and frame counters, so the library's copy is duplicate output
// that the process has to write from the decode thread.
fn silence_library_logging() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| av::util::log::set_level(av::util::log::Level::Fatal));
}

impl Decoder {
    pub fn recover(&mut self) -> Result<(), String> {
        let raw: ffi::AVCodecID = self.kind.id().into();
        let parser = NonNull::new(unsafe { ffi::av_parser_init(raw) })
            .ok_or_else(|| "Could not reset broadcast parser".to_owned())?;
        unsafe {
            ffi::av_parser_close(self.parser.as_ptr());
        }
        self.parser = parser;
        self.pending_bytes = 0;
        self.codec.flush();
        self.resampler = None;
        self.audio_spec = None;
        self.next_pts = None;
        Ok(())
    }

    pub fn new(kind: Kind) -> Result<Self, String> {
        silence_library_logging();
        let id = kind.id();
        let codec =
            av::decoder::find(id).ok_or_else(|| format!("Decoder unavailable: {kind:?}"))?;
        let mut context = codec::Context::new_with_codec(codec);
        unsafe {
            (*context.as_mut_ptr()).max_pixels = 4096 * 2160;
            (*context.as_mut_ptr()).pkt_timebase = ffi::AVRational { num: 1, den: 90000 };
        }
        context.set_threading(av::threading::Config {
            count: 1,
            ..Default::default()
        });
        let decoder = context.decoder();
        let opened = decoder.open_as(codec).map_err(|e| e.to_string())?;
        let raw: ffi::AVCodecID = id.into();
        let parser = NonNull::new(unsafe { ffi::av_parser_init(raw) })
            .ok_or_else(|| format!("Parser unavailable: {kind:?}"))?;
        Ok(Self {
            kind,
            next_pts: None,
            pending_bytes: 0,
            parser,
            codec: opened,
            resampler: None,
            scaler: None,
            audio_spec: None,
            video_spec: None,
        })
    }

    pub fn push(
        &mut self,
        bytes: &[u8],
        pts: Option<i64>,
        out: &mut Vec<(Option<i64>, Payload)>,
    ) -> Result<(), String> {
        let mut padded =
            Vec::with_capacity(bytes.len() + ffi::AV_INPUT_BUFFER_PADDING_SIZE as usize);
        padded.extend_from_slice(bytes);
        padded.resize(padded.capacity(), 0);
        let mut used = 0;
        while used < bytes.len() {
            let mut packet_data = std::ptr::null_mut();
            let mut packet_size = 0;
            let consumed = unsafe {
                ffi::av_parser_parse2(
                    self.parser.as_ptr(),
                    self.codec.as_mut_ptr(),
                    &mut packet_data,
                    &mut packet_size,
                    padded[used..].as_ptr(),
                    (bytes.len() - used) as i32,
                    if used == 0 {
                        pts.unwrap_or(ffi::AV_NOPTS_VALUE)
                    } else {
                        ffi::AV_NOPTS_VALUE
                    },
                    ffi::AV_NOPTS_VALUE,
                    -1,
                )
            };
            if consumed < 0 || (consumed == 0 && packet_size == 0) {
                return Err(format!(
                    "Invalid {kind:?} elementary stream",
                    kind = self.kind
                ));
            }
            self.pending_bytes = self.pending_bytes.saturating_add(consumed.max(0) as usize);
            if self.pending_bytes > 8 * 1024 * 1024 || packet_size > 8 * 1024 * 1024 {
                return Err("Broadcast elementary-stream frame exceeds 8 MiB".to_owned());
            }
            if packet_size > 0 {
                self.pending_bytes = 0;
                let mut packet = av::Packet::copy(unsafe {
                    std::slice::from_raw_parts(packet_data, packet_size as usize)
                });
                let parsed_pts = unsafe { self.parser.as_ref().pts };
                packet.set_pts((parsed_pts != ffi::AV_NOPTS_VALUE).then_some(parsed_pts));
                self.codec.send_packet(&packet).map_err(|e| e.to_string())?;
                self.receive(out)?;
            }
            used += consumed as usize;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn finish(&mut self, out: &mut Vec<(Option<i64>, Payload)>) -> Result<(), String> {
        let mut data = std::ptr::null_mut();
        let mut length = 0;
        unsafe {
            ffi::av_parser_parse2(
                self.parser.as_ptr(),
                self.codec.as_mut_ptr(),
                &mut data,
                &mut length,
                std::ptr::null(),
                0,
                ffi::AV_NOPTS_VALUE,
                ffi::AV_NOPTS_VALUE,
                -1,
            );
        }
        if length > 0 {
            let packet =
                av::Packet::copy(unsafe { std::slice::from_raw_parts(data, length as usize) });
            self.codec.send_packet(&packet).map_err(|e| e.to_string())?;
            self.receive(out)?;
        }
        self.codec.send_eof().map_err(|e| e.to_string())?;
        self.receive(out)
    }

    fn receive(&mut self, out: &mut Vec<(Option<i64>, Payload)>) -> Result<(), String> {
        loop {
            if self.kind.video() {
                let mut frame = frame::Video::empty();
                match self.codec.receive_frame(&mut frame) {
                    Ok(()) => {
                        let rate = self
                            .codec
                            .frame_rate()
                            .filter(|rate| rate.numerator() > 0 && rate.denominator() > 0);
                        let duration = rate.map_or(3600, |rate| {
                            90000 * i64::from(rate.denominator()) / i64::from(rate.numerator())
                        });
                        let pts = frame.pts().or(self.next_pts);
                        self.next_pts = pts.map(|pts| pts + duration.max(1));
                        out.push((pts, Payload::Video(self.picture(&frame)?)));
                    }
                    Err(av::Error::Eof) => return Ok(()),
                    Err(av::Error::Other { errno })
                        if std::io::Error::from_raw_os_error(errno).kind()
                            == std::io::ErrorKind::WouldBlock =>
                    {
                        return Ok(());
                    }
                    Err(error) => return Err(error.to_string()),
                }
            } else {
                let mut frame = frame::Audio::empty();
                match self.codec.receive_frame(&mut frame) {
                    Ok(()) => {
                        let pcm = self.samples(&frame)?;
                        let pts = frame.pts().or(self.next_pts);
                        self.next_pts = pts.map(|pts| {
                            pts + 90000 * frame.samples() as i64 / i64::from(frame.rate())
                        });
                        out.push((pts, Payload::Audio(pcm)));
                    }
                    Err(av::Error::Eof) => return Ok(()),
                    Err(av::Error::Other { errno })
                        if std::io::Error::from_raw_os_error(errno).kind()
                            == std::io::ErrorKind::WouldBlock =>
                    {
                        return Ok(());
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
    }

    fn samples(&mut self, frame: &frame::Audio) -> Result<Vec<f32>, String> {
        let spec = (frame.format(), frame.rate(), frame.ch_layout().mask());
        if !(8000..=192000).contains(&frame.rate())
            || frame.samples() > 8192
            || !(1..=8).contains(&frame.ch_layout().channels())
        {
            return Err("Unsupported broadcast audio geometry".to_owned());
        }
        if self.audio_spec != Some(spec) {
            self.resampler = Some(
                software::resampling::Context::get2(
                    frame.format(),
                    frame.ch_layout(),
                    frame.rate(),
                    Sample::F32(Type::Packed),
                    av::ChannelLayout::STEREO,
                    AUDIO_RATE,
                )
                .map_err(|e| e.to_string())?,
            );
            self.audio_spec = Some(spec);
        }
        let capacity = (frame.samples() * AUDIO_RATE as usize / frame.rate() as usize) + 256;
        let mut converted = frame::Audio::new(
            Sample::F32(Type::Packed),
            capacity,
            av::ChannelLayoutMask::STEREO,
        );
        converted.set_rate(AUDIO_RATE);
        let Some(resampler) = &mut self.resampler else {
            return Err("Audio resampler missing".to_owned());
        };
        resampler
            .run(frame, &mut converted)
            .map_err(|e| e.to_string())?;
        let mut pcm = Vec::with_capacity(converted.samples() * 2);
        for &(left, right) in converted.plane::<(f32, f32)>(0) {
            if !left.is_finite() || !right.is_finite() {
                return Err("Non-finite broadcast audio".to_owned());
            }
            pcm.extend_from_slice(&[left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0)]);
        }
        Ok(pcm)
    }

    fn picture(&mut self, frame: &frame::Video) -> Result<VideoPicture, String> {
        let aspect = frame.aspect_ratio();
        let width = if aspect.numerator() > 0 && aspect.denominator() > 0 {
            (u64::from(frame.width()) * aspect.numerator() as u64 + aspect.denominator() as u64 / 2)
                / aspect.denominator() as u64
        } else {
            u64::from(frame.width())
        };
        if width == 0 || width > 4096 {
            return Err("Unsupported broadcast video display width".to_owned());
        }
        let spec = (frame.format(), frame.width(), frame.height(), width as u32);
        if spec.1 == 0 || spec.2 == 0 || spec.1 > 4096 || spec.2 > 2160 {
            return Err("Unsupported broadcast video dimensions".to_owned());
        }
        if self.video_spec != Some(spec) {
            self.scaler = Some(
                software::scaling::Context::get(
                    spec.0,
                    spec.1,
                    spec.2,
                    Pixel::RGB24,
                    spec.3,
                    spec.2,
                    software::scaling::Flags::BILINEAR,
                )
                .map_err(|e| e.to_string())?,
            );
            self.video_spec = Some(spec);
        }
        let Some(scaler) = &mut self.scaler else {
            return Err("Video scaler missing".to_owned());
        };
        let mut rgb_frame = frame::Video::empty();
        scaler
            .run(frame, &mut rgb_frame)
            .map_err(|e| e.to_string())?;
        let width = spec.3 as usize;
        let height = spec.2 as usize;
        let mut rgb = Vec::with_capacity(width * height * 3);
        for row in rgb_frame.data(0).chunks(rgb_frame.stride(0)).take(height) {
            rgb.extend_from_slice(&row[..width * 3]);
        }
        Ok(VideoPicture {
            width: spec.3 as u16,
            height: spec.2 as u16,
            luma: Vec::new(),
            rgb,
        })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            ffi::av_parser_close(self.parser.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anamorphic_frames_are_resampled_to_square_pixels() {
        let mut decoder = Decoder::new(Kind::Mpeg2).expect("decoder");
        let mut source = frame::Video::new(Pixel::RGB24, 720, 576);
        source.data_mut(0).fill(127);
        unsafe {
            (*source.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 64, den: 45 };
        }
        let picture = decoder.picture(&source).expect("picture");
        assert_eq!((picture.width, picture.height), (1024, 576));
        assert_eq!(picture.rgb.len(), 1024 * 576 * 3);
        assert!(picture.rgb.iter().all(|&sample| sample.abs_diff(127) <= 1));
        unsafe {
            (*source.as_mut_ptr()).sample_aspect_ratio = ffi::AVRational { num: 1000, den: 1 };
        }
        assert!(decoder.picture(&source).is_err());
    }
}

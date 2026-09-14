use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sdrmm_device::{DeviceError, Direction, DuplexState, Sample, TxStream, lock};

use crate::{
    convert::to_elements,
    iio::{Link, Response, close_buffer, mask, open_buffer, set_remote_timeout, write_buf},
    layout::Stream,
    source::Source,
};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_TIMEOUT: Duration = Duration::from_secs(2);

/// The transmit buffer of one radio, held open for as long as the stream is.
pub(crate) struct Ad936xTx {
    link: Option<Link>,
    device: String,
    stream: Stream,
    lanes: usize,
    bytes: Vec<u8>,
    frame: Vec<Sample>,
    duplex: Arc<Mutex<DuplexState>>,
}

impl Ad936xTx {
    pub(crate) fn open(
        source: &Source,
        stream: &Stream,
        lanes: usize,
        samples: usize,
        duplex: Arc<Mutex<DuplexState>>,
    ) -> Result<Self, DeviceError> {
        let mut link = Link::new(source.open()?);
        set_remote_timeout(&mut link, REMOTE_TIMEOUT)?;
        let elements = stream.elements(lanes);
        open_buffer(
            &mut link,
            &stream.device,
            samples,
            &mask(&elements, stream.scan_total),
        )?;
        tracing::debug!(
            device = stream.device,
            lanes,
            samples,
            "ad936x transmit buffer opened"
        );
        Ok(Self {
            link: Some(link),
            device: stream.device.clone(),
            stream: stream.clone(),
            lanes,
            bytes: Vec::new(),
            frame: Vec::new(),
            duplex,
        })
    }

    /// Sends one buffer's worth and reports what the radio took.
    ///
    /// IIOD acknowledges the request before the payload and again after it, so a buffer the radio
    /// will not take costs nothing but the first answer.
    fn send(&mut self) -> Result<usize, DeviceError> {
        let Some(link) = self.link.as_mut() else {
            return Err(DeviceError::Io("transmit stream is stopped".to_string()));
        };
        link.send(&write_buf(&self.device, self.bytes.len()))?;
        answer(link, "offer a transmit buffer")?;
        link.transport().send(&self.bytes)?;
        answer(link, "hand over a transmit buffer")
    }

    fn release(&mut self) {
        if let Some(mut link) = self.link.take() {
            close_buffer(&mut link, &self.device);
            link.close();
        }
        lock(&self.duplex).release(Direction::Tx);
    }
}

fn answer(link: &mut Link, what: &str) -> Result<usize, DeviceError> {
    let line = link.read_line(WRITE_TIMEOUT)?;
    Response::parse(&line)
        .ok_or_else(|| DeviceError::Io(format!("{what}: iiod answered {line:?}")))?
        .bytes(what)
}

/// Lays the lanes out the way the buffer carries them: one sample of each, in lane order.
fn interleave(channels: &[&[Sample]], out: &mut Vec<Sample>) -> usize {
    let span = channels.iter().map(|lane| lane.len()).min().unwrap_or(0);
    out.clear();
    out.reserve(span * channels.len());
    for index in 0..span {
        for lane in channels {
            out.push(lane[index]);
        }
    }
    span
}

impl TxStream for Ad936xTx {
    fn write_channels(
        &mut self,
        channels: &[&[Sample]],
        _timeout: Duration,
        _end_burst: bool,
    ) -> Result<usize, DeviceError> {
        if channels.len() != self.lanes {
            return Err(DeviceError::Unsupported(format!(
                "this radio transmits on {} lanes, got {}",
                self.lanes,
                channels.len()
            )));
        }
        let span = interleave(channels, &mut self.frame);
        if span == 0 {
            return Ok(0);
        }
        let frame = std::mem::take(&mut self.frame);
        to_elements(&frame, self.stream.format, &mut self.bytes);
        self.frame = frame;
        let accepted = self.send()?;
        Ok(accepted / self.stream.sample_bytes(self.lanes).max(1))
    }

    fn stop(&mut self) -> Result<(), DeviceError> {
        self.release();
        Ok(())
    }
}

impl Drop for Ad936xTx {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(values: &[f32]) -> Vec<Sample> {
        values.iter().map(|v| Sample::new(*v, 0.0)).collect()
    }

    #[test]
    fn lanes_are_laid_out_one_sample_of_each_in_turn() {
        let first = samples(&[1.0, 3.0, 5.0]);
        let second = samples(&[2.0, 4.0, 6.0]);
        let mut out = Vec::new();
        let span = interleave(&[&first, &second], &mut out);
        assert_eq!(span, 3);
        assert_eq!(
            out.iter().map(|s| s.re).collect::<Vec<_>>(),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
        );
    }

    #[test]
    fn lanes_of_different_lengths_send_only_what_every_lane_has() {
        let first = samples(&[1.0, 3.0, 5.0]);
        let second = samples(&[2.0]);
        let mut out = Vec::new();
        assert_eq!(interleave(&[&first, &second], &mut out), 1);
        assert_eq!(out.iter().map(|s| s.re).collect::<Vec<_>>(), vec![1.0, 2.0]);
    }

    #[test]
    fn one_lane_is_laid_out_as_it_came() {
        let only = samples(&[1.0, 2.0]);
        let mut out = Vec::new();
        assert_eq!(interleave(&[&only], &mut out), 2);
        assert_eq!(out, only);
    }

    #[test]
    fn nothing_to_send_is_not_an_error() {
        let empty: Vec<Sample> = Vec::new();
        let mut out = Vec::new();
        assert_eq!(interleave(&[&empty], &mut out), 0);
        assert!(out.is_empty());
        assert_eq!(interleave(&[], &mut out), 0);
    }
}

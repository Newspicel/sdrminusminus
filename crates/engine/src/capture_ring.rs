use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};

const SPAN_CAPACITY: usize = 4096;

#[derive(Clone, Copy)]
struct Span {
    start: u64,
    len: usize,
}

pub(crate) struct CaptureProducer {
    samples: Producer<Complex<f32>>,
    spans: Producer<Span>,
}

pub(crate) struct CaptureConsumer {
    samples: Consumer<Complex<f32>>,
    spans: Consumer<Span>,
    pending: Option<Span>,
}

pub(crate) fn capture_ring(capacity: usize) -> (CaptureProducer, CaptureConsumer) {
    let (samples_tx, samples_rx) = RingBuffer::new(capacity);
    let (spans_tx, spans_rx) = RingBuffer::new(capacity.min(SPAN_CAPACITY));
    (
        CaptureProducer {
            samples: samples_tx,
            spans: spans_tx,
        },
        CaptureConsumer {
            samples: samples_rx,
            spans: spans_rx,
            pending: None,
        },
    )
}

impl CaptureProducer {
    pub(crate) fn push(&mut self, samples: &[Complex<f32>], start: u64) -> usize {
        let len = samples.len().min(self.samples.slots());
        if len == 0 {
            return 0;
        }
        let Ok(span) = self.spans.write_chunk_uninit(1) else {
            return 0;
        };
        let Ok(chunk) = self.samples.write_chunk_uninit(len) else {
            return 0;
        };
        chunk.fill_from_iter(samples[..len].iter().copied());
        span.fill_from_iter([Span { start, len }]);
        len
    }
}

impl CaptureConsumer {
    pub(crate) fn consume(
        &mut self,
        limit: usize,
        mut receive: impl FnMut(&[Complex<f32>], u64),
    ) -> usize {
        let Some(mut span) = self.pending.take().or_else(|| self.spans.pop().ok()) else {
            return 0;
        };
        let len = span.len.min(limit);
        let Ok(chunk) = self.samples.read_chunk(len) else {
            self.pending = Some(span);
            return 0;
        };
        let (a, b) = chunk.as_slices();
        if !a.is_empty() {
            receive(a, span.start);
        }
        if !b.is_empty() {
            receive(b, span.start + a.len() as u64);
        }
        chunk.commit_all();
        span.start += len as u64;
        span.len -= len;
        self.pending = (span.len > 0).then_some(span);
        len
    }
}

#[cfg(test)]
mod tests;

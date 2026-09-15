use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceError, StopHandle, StreamFailure,
    net::{Read, SocketStop},
};

use crate::iio::proto::Response;

/// Bytes pulled from the transport in one go. A multiple of every bulk packet size a full-speed
/// or high-speed endpoint can have, so a USB transfer is never refused for its length.
const BUFFER: usize = 65_536;

const MAX_LINE: usize = 1024;

/// A payload at least this long is read straight into the caller's slice rather than staged
/// through the buffer, so a sample block costs one copy rather than two.
const DIRECT_MIN: usize = 4096;

/// One IIOD conversation: a socket, or one endpoint couple of the USB interface.
///
/// Commands and their answers are strictly ordered on it, so a radio that is streaming holds one
/// of these for the buffer and another for everything else.
pub(crate) trait Transport: Send + Sync + 'static {
    fn send(&self, bytes: &[u8]) -> Result<(), DeviceError>;
    /// Reads into `buf`, done as soon as `wanted` bytes are in hand. A transport that moves
    /// whole packets may hand over more than `wanted`, never more than `buf` holds.
    fn read(&self, buf: &mut [u8], wanted: usize, timeout: Duration) -> Read;
    fn fail(&self, reason: String);
    fn failure(&self) -> StreamFailure;
    fn close(&self);
    fn stopper(&self) -> Stopper;
}

/// Ends a parked read from outside the thread doing it.
///
/// A socket is shut down, which wakes the read at once; a USB endpoint has no such door, so its
/// reads look for the flag while they wait and cancel their transfer when it is raised.
#[derive(Clone, Debug, Default)]
pub(crate) struct Stopper {
    socket: Option<SocketStop>,
    stopped: Arc<AtomicBool>,
}

impl Stopper {
    pub(crate) fn socket(stop: SocketStop) -> Self {
        Self {
            socket: Some(stop),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn flag() -> Self {
        Self::default()
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

impl StopHandle for Stopper {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(socket) = &self.socket {
            socket.stop();
        }
    }
}

/// A transport read as lines and byte counts, which is the only shape IIOD answers in.
pub(crate) struct Link {
    transport: Arc<dyn Transport>,
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link")
            .field("buffered", &(self.end - self.start))
            .finish_non_exhaustive()
    }
}

impl Link {
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            buf: vec![0u8; BUFFER],
            start: 0,
            end: 0,
        }
    }

    pub(crate) fn transport(&self) -> &Arc<dyn Transport> {
        &self.transport
    }

    pub(crate) fn stopper(&self) -> Stopper {
        self.transport.stopper()
    }

    pub(crate) fn failure(&self) -> StreamFailure {
        self.transport.failure()
    }

    pub(crate) fn close(&self) {
        self.transport.close();
    }

    pub(crate) fn send(&self, command: &str) -> Result<(), DeviceError> {
        tracing::trace!(command = command.trim_end(), "iiod command");
        self.transport.send(command.as_bytes())
    }

    fn buffered(&self) -> &[u8] {
        &self.buf[self.start..self.end]
    }

    fn consume(&mut self, bytes: usize) {
        self.start += bytes;
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        }
    }

    /// Pulls one transport read into the buffer, moving whatever is left over to the front first.
    fn fill(&mut self, wanted: usize, timeout: Duration) -> Read {
        if self.start > 0 {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
        }
        if self.end == self.buf.len() {
            return Read::Idle;
        }
        match self
            .transport
            .read(&mut self.buf[self.end..], wanted, timeout)
        {
            Read::Got(n) => {
                self.end += n;
                Read::Got(n)
            }
            other => other,
        }
    }

    /// Takes whatever is already here, pulling once if nothing is. Never waits past `timeout`, so
    /// a capture thread stays responsive to a stop between the pieces of one answer.
    pub(crate) fn take(&mut self, dst: &mut [u8], timeout: Duration) -> Read {
        if self.buffered().is_empty() {
            if dst.len() >= DIRECT_MIN {
                return self.transport.read(dst, dst.len(), timeout);
            }
            if let ended @ (Read::Idle | Read::Ended) = self.fill(dst.len(), timeout) {
                return ended;
            }
        }
        let taken = self.buffered().len().min(dst.len());
        dst[..taken].copy_from_slice(&self.buffered()[..taken]);
        self.consume(taken);
        Read::Got(taken)
    }

    /// One answer line, without its terminator. Blank lines are skipped: IIOD ends some answers
    /// with a newline of their own before the next line begins.
    pub(crate) fn read_line(&mut self, timeout: Duration) -> Result<String, DeviceError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(line) = self.next_line()? {
                return Ok(line);
            }
            self.wait(deadline, "an answer line")?;
        }
    }

    /// One answer line if it arrives within `timeout`, and nothing rather than a broken link if
    /// it does not, so a caller polling between blocks can come back for it.
    pub(crate) fn poll_line(&mut self, timeout: Duration) -> Result<Option<String>, DeviceError> {
        if let Some(line) = self.next_line()? {
            return Ok(Some(line));
        }
        match self.fill(1, timeout) {
            Read::Got(_) => self.next_line(),
            Read::Idle => Ok(None),
            Read::Ended => Err(self.ended()),
        }
    }

    fn next_line(&mut self) -> Result<Option<String>, DeviceError> {
        loop {
            let Some(end) = self.buffered().iter().position(|byte| *byte == b'\n') else {
                if self.buffered().len() > MAX_LINE {
                    return Err(self.broken("iiod sent an answer line with no end to it"));
                }
                return Ok(None);
            };
            let line = String::from_utf8_lossy(&self.buffered()[..end])
                .trim_end_matches('\r')
                .to_string();
            self.consume(end + 1);
            if !line.trim().is_empty() {
                return Ok(Some(line));
            }
        }
    }

    /// The byte count IIOD answers a command with, or the refusal it answered instead.
    pub(crate) fn answer(&mut self, what: &str, timeout: Duration) -> Result<usize, DeviceError> {
        let line = self.read_line(timeout)?;
        parse_answer(&line, what)
    }

    pub(crate) fn read_exact(
        &mut self,
        dst: &mut [u8],
        timeout: Duration,
    ) -> Result<(), DeviceError> {
        let deadline = Instant::now() + timeout;
        let mut got = 0;
        while got < dst.len() {
            match self.take(&mut dst[got..], remaining(deadline)) {
                Read::Got(n) => got += n,
                Read::Idle => {
                    self.at_deadline(deadline, "the bytes iiod said would follow")?;
                }
                Read::Ended => return Err(self.ended()),
            }
        }
        Ok(())
    }

    /// Drops bytes the caller has no room for, so a mismatched answer costs one read rather than
    /// the whole conversation.
    pub(crate) fn discard(
        &mut self,
        mut bytes: usize,
        timeout: Duration,
    ) -> Result<(), DeviceError> {
        let mut sink = [0u8; 512];
        let deadline = Instant::now() + timeout;
        while bytes > 0 {
            let want = bytes.min(sink.len());
            match self.take(&mut sink[..want], remaining(deadline)) {
                Read::Got(n) => bytes -= n,
                Read::Idle => self.at_deadline(deadline, "the bytes iiod said would follow")?,
                Read::Ended => return Err(self.ended()),
            }
        }
        Ok(())
    }

    fn wait(&mut self, deadline: Instant, what: &str) -> Result<(), DeviceError> {
        match self.fill(1, remaining(deadline)) {
            Read::Got(_) => Ok(()),
            Read::Idle => self.at_deadline(deadline, what),
            Read::Ended => Err(self.ended()),
        }
    }

    fn at_deadline(&mut self, deadline: Instant, what: &str) -> Result<(), DeviceError> {
        if Instant::now() >= deadline {
            return Err(self.broken(&format!("the radio did not send {what} in time")));
        }
        Ok(())
    }

    fn broken(&self, reason: &str) -> DeviceError {
        self.transport.fail(reason.to_string());
        DeviceError::Io(reason.to_string())
    }

    pub(crate) fn ended(&self) -> DeviceError {
        let failure = self.transport.failure();
        if failure.gone {
            DeviceError::Disconnected(failure.reason)
        } else {
            DeviceError::Io(failure.reason)
        }
    }
}

pub(crate) fn parse_answer(line: &str, what: &str) -> Result<usize, DeviceError> {
    Response::parse(line)
        .ok_or_else(|| DeviceError::Io(format!("{what}: iiod answered {line:?}")))?
        .bytes(what)
}

pub(crate) fn remaining(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use sdrmm_device::lock;

    use super::*;

    /// A transport that answers from a script, one chunk per read, and is quiet once it runs out.
    #[derive(Debug, Default)]
    pub(crate) struct Scripted {
        chunks: Mutex<Vec<Vec<u8>>>,
        pub(crate) sent: Mutex<Vec<u8>>,
        failure: Mutex<Option<String>>,
    }

    impl Scripted {
        pub(crate) fn with(chunks: &[&[u8]]) -> Arc<Self> {
            let scripted = Arc::new(Self::default());
            scripted.feed(chunks);
            scripted
        }

        pub(crate) fn feed(&self, chunks: &[&[u8]]) {
            let mut queued = lock(&self.chunks);
            for chunk in chunks {
                queued.insert(0, chunk.to_vec());
            }
        }

        pub(crate) fn failed(&self) -> Option<String> {
            lock(&self.failure).clone()
        }
    }

    impl Transport for Scripted {
        fn send(&self, bytes: &[u8]) -> Result<(), DeviceError> {
            lock(&self.sent).extend_from_slice(bytes);
            Ok(())
        }

        fn read(&self, buf: &mut [u8], _wanted: usize, _timeout: Duration) -> Read {
            let mut chunks = lock(&self.chunks);
            let Some(chunk) = chunks.last_mut() else {
                return Read::Idle;
            };
            let taken = chunk.len().min(buf.len());
            buf[..taken].copy_from_slice(&chunk[..taken]);
            chunk.drain(..taken);
            if chunk.is_empty() {
                chunks.pop();
            }
            Read::Got(taken)
        }

        fn fail(&self, reason: String) {
            let mut failure = lock(&self.failure);
            if failure.is_none() {
                *failure = Some(reason);
            }
        }

        fn failure(&self) -> StreamFailure {
            StreamFailure {
                reason: lock(&self.failure)
                    .clone()
                    .unwrap_or_else(|| "ended".to_string()),
                gone: false,
            }
        }

        fn close(&self) {}

        fn stopper(&self) -> Stopper {
            Stopper::flag()
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_device::lock;

    use super::{testing::Scripted, *};

    fn link(chunks: &[&[u8]]) -> Link {
        Link::new(Scripted::with(chunks))
    }

    const SHORT: Duration = Duration::from_millis(50);

    #[test]
    fn lines_come_back_one_at_a_time_whatever_the_chunking() {
        let mut link = link(&[b"12", b"8\r\nab", b"cd\n"]);
        assert_eq!(link.read_line(SHORT).expect("first"), "128");
        let mut data = [0u8; 4];
        link.read_exact(&mut data, SHORT).expect("payload");
        assert_eq!(&data, b"abcd");
    }

    #[test]
    fn a_blank_line_between_answers_is_not_an_answer() {
        let mut link = link(&[b"\n\n-22\n"]);
        assert_eq!(link.read_line(SHORT).expect("line"), "-22");
    }

    #[test]
    fn a_line_that_never_ends_is_refused_rather_than_buffered_forever() {
        let filler = vec![b'x'; MAX_LINE + 8];
        let mut link = link(&[&filler]);
        let error = link.read_line(SHORT).expect_err("refused");
        assert!(error.to_string().contains("no end"), "{error}");
    }

    #[test]
    fn a_quiet_transport_times_out_instead_of_parking_forever() {
        let mut link = link(&[]);
        let started = Instant::now();
        let error = link.read_line(SHORT).expect_err("timed out");
        assert!(error.to_string().contains("in time"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn polling_for_a_line_that_has_not_come_leaves_the_link_whole() {
        let transport = Scripted::with(&[]);
        let mut link = Link::new(transport.clone());
        assert_eq!(link.poll_line(SHORT).expect("quiet"), None);
        assert_eq!(
            transport.failed(),
            None,
            "a quiet radio is not a broken one"
        );
        transport.feed(&[b"40", b"96\n"]);
        assert_eq!(
            link.poll_line(SHORT).expect("half a line"),
            None,
            "a line is not handed over until it ends"
        );
        assert_eq!(
            link.poll_line(SHORT).expect("line"),
            Some("4096".to_string()),
            "and the half that came first is not lost"
        );
    }

    #[test]
    fn an_answer_is_the_count_it_carries_or_the_refusal_it_is() {
        let mut link = link(&[b"4096\n-22\nwhat\n"]);
        assert_eq!(link.answer("refill", SHORT).expect("count"), 4096);
        assert!(matches!(
            link.answer("set rate", SHORT),
            Err(DeviceError::Unsupported(_))
        ));
        let error = link.answer("read", SHORT).expect_err("not a number");
        assert!(error.to_string().contains("what"), "{error}");
    }

    #[test]
    fn take_hands_over_what_is_buffered_without_waiting_for_the_rest() {
        let mut link = link(&[b"abcdef"]);
        let mut dst = [0u8; 4];
        assert_eq!(link.take(&mut dst, SHORT), Read::Got(4));
        assert_eq!(&dst, b"abcd");
        assert_eq!(link.take(&mut dst, SHORT), Read::Got(2));
        assert_eq!(&dst[..2], b"ef");
        assert_eq!(link.take(&mut dst, SHORT), Read::Idle);
    }

    #[test]
    fn a_large_payload_is_read_straight_into_its_destination() {
        let payload: Vec<u8> = (0..DIRECT_MIN * 2).map(|n| n as u8).collect();
        let mut link = link(&[b"8192\n", &payload, b"0\n"]);
        assert_eq!(link.answer("refill", SHORT).expect("count"), payload.len());
        let mut dst = vec![0u8; payload.len()];
        link.read_exact(&mut dst, SHORT).expect("payload");
        assert_eq!(dst, payload);
        assert_eq!(link.answer("refill", SHORT).expect("end"), 0);
    }

    #[test]
    fn discarding_skips_exactly_what_was_asked_for() {
        let mut link = link(&[b"0123456789rest\n"]);
        link.discard(10, SHORT).expect("discard");
        assert_eq!(link.read_line(SHORT).expect("line"), "rest");
    }

    #[test]
    fn a_command_reaches_the_transport_verbatim() {
        let transport = Scripted::with(&[]);
        let link = Link::new(transport.clone());
        link.send("VERSION\r\n").expect("send");
        assert_eq!(&*lock(&transport.sent), b"VERSION\r\n");
    }

    #[test]
    fn a_stopper_without_a_socket_still_reports_that_it_was_stopped() {
        let stopper = Stopper::flag();
        assert!(!stopper.is_stopped());
        stopper.stop();
        assert!(stopper.is_stopped());
    }
}

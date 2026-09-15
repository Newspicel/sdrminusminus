use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use sdrmm_device::{DeviceError, lock};

use crate::iio::{
    link::{Link, Transport},
    proto,
    xml::Context,
};

/// How long a control exchange may take. Attribute writes reach the AD936x over SPI and a retune
/// recalibrates, so this is generous by the standards of the rest of the conversation.
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// The largest context description this driver will take. An AD936x context is around 30 kB.
const MAX_XML: usize = 4 * 1024 * 1024;

const MAX_ATTR: usize = 16 * 1024;

/// Which end of a channel pair an attribute belongs to. IIOD names receive channels INPUT and
/// transmit channels OUTPUT, and the same channel id exists on both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    In,
    Out,
}

impl Direction {
    const fn is_output(self) -> bool {
        matches!(self, Self::Out)
    }
}

/// Sends one command and takes the answer line that follows it.
pub(crate) fn exec(
    link: &mut Link,
    command: &str,
    what: &str,
    timeout: Duration,
) -> Result<usize, DeviceError> {
    link.send(command)?;
    link.answer(what, timeout)
}

pub(crate) fn open_buffer(
    link: &mut Link,
    device: &str,
    samples: usize,
    mask: &[u32],
) -> Result<(), DeviceError> {
    exec(
        link,
        &proto::open(device, samples, mask),
        "open the sample buffer",
        CONTROL_TIMEOUT,
    )
    .map(drop)
}

pub(crate) fn close_buffer(link: &mut Link, device: &str) {
    if let Err(e) = exec(
        link,
        &proto::close(device),
        "close the sample buffer",
        CONTROL_TIMEOUT,
    ) {
        tracing::debug!("closing the {device} buffer: {e}");
    }
}

/// Tells the radio how long to wait for its own hardware before answering, so a stalled buffer
/// comes back as a refusal instead of leaving the host waiting on a read that never ends.
pub(crate) fn set_remote_timeout(link: &mut Link, timeout: Duration) -> Result<(), DeviceError> {
    let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    exec(
        link,
        &proto::timeout(ms),
        "set the radio's own timeout",
        CONTROL_TIMEOUT,
    )
    .map(drop)
}

/// The control conversation with one radio: everything that is not a buffer.
pub(crate) struct Client {
    link: Mutex<Link>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Client")
    }
}

impl Client {
    pub(crate) fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            link: Mutex::new(Link::new(transport)),
        }
    }

    pub(crate) fn close(&self) {
        lock(&self.link).close();
    }

    /// The libiio version the radio serves, for the log line that says what was found.
    pub(crate) fn version(&self) -> Result<String, DeviceError> {
        let mut link = lock(&self.link);
        link.send(&proto::version())?;
        link.read_line(CONTROL_TIMEOUT)
    }

    pub(crate) fn context(&self) -> Result<Context, DeviceError> {
        self.context_within(CONTROL_TIMEOUT)
    }

    /// The context with a patience of the caller's choosing, for a search that asks hosts nobody
    /// said were radios.
    pub(crate) fn context_within(&self, timeout: Duration) -> Result<Context, DeviceError> {
        let mut link = lock(&self.link);
        let bytes = exec(
            &mut link,
            &proto::print(),
            "ask the radio what it is",
            timeout,
        )?;
        if bytes > MAX_XML {
            return Err(oversized(
                &mut link,
                bytes,
                format!(
                    "the radio described itself in {bytes} bytes, which is more than this driver \
                     reads"
                ),
            ));
        }
        let mut xml = vec![0u8; bytes + 1];
        link.read_exact(&mut xml, timeout)?;
        xml.truncate(bytes);
        Context::parse(&String::from_utf8_lossy(&xml))
    }

    pub(crate) fn read_device_attr(&self, device: &str, attr: &str) -> Result<String, DeviceError> {
        let mut link = lock(&self.link);
        let command = proto::read_device_attr(device, attr);
        let what = format!("read {device}.{attr}");
        take_attr(&mut link, &command, &what)
    }

    pub(crate) fn write_device_attr(
        &self,
        device: &str,
        attr: &str,
        value: &str,
    ) -> Result<(), DeviceError> {
        let mut link = lock(&self.link);
        let payload = payload(value);
        let command = proto::write_device_attr(device, attr, payload.len());
        put_attr(
            &mut link,
            &command,
            &payload,
            &format!("set {device}.{attr}"),
        )
    }

    pub(crate) fn read_channel_attr(
        &self,
        device: &str,
        direction: Direction,
        channel: &str,
        attr: &str,
    ) -> Result<String, DeviceError> {
        let mut link = lock(&self.link);
        let command = proto::read_channel_attr(device, direction.is_output(), channel, attr);
        let what = format!("read {device}.{channel}.{attr}");
        take_attr(&mut link, &command, &what)
    }

    pub(crate) fn write_channel_attr(
        &self,
        device: &str,
        direction: Direction,
        channel: &str,
        attr: &str,
        value: &str,
    ) -> Result<(), DeviceError> {
        let mut link = lock(&self.link);
        let payload = payload(value);
        let command =
            proto::write_channel_attr(device, direction.is_output(), channel, attr, payload.len());
        put_attr(
            &mut link,
            &command,
            &payload,
            &format!("set {device}.{channel}.{attr}"),
        )
    }
}

/// Steps over an answer this driver will not hold, so the link stays usable, and reports why.
/// A radio that will not even hand the bytes over has broken the conversation either way, so the
/// reason the answer was refused is the one that survives.
fn oversized(link: &mut Link, bytes: usize, reason: String) -> DeviceError {
    if let Err(e) = link.discard(bytes + 1, CONTROL_TIMEOUT) {
        tracing::debug!("skipping an oversized answer: {e}");
    }
    DeviceError::Io(reason)
}

/// Attribute values travel with their terminator, which is what the kernel side writes and reads.
fn payload(value: &str) -> Vec<u8> {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

fn take_attr(link: &mut Link, command: &str, what: &str) -> Result<String, DeviceError> {
    let bytes = exec(link, command, what, CONTROL_TIMEOUT)?;
    if bytes > MAX_ATTR {
        return Err(oversized(
            link,
            bytes,
            format!("{what}: {bytes} bytes is not a value"),
        ));
    }
    let mut value = vec![0u8; bytes + 1];
    link.read_exact(&mut value, CONTROL_TIMEOUT)?;
    value.truncate(bytes);
    Ok(String::from_utf8_lossy(&value)
        .trim_end_matches(['\0', '\n', '\r'])
        .to_string())
}

fn put_attr(link: &mut Link, command: &str, payload: &[u8], what: &str) -> Result<(), DeviceError> {
    link.send(command)?;
    link.transport().send(payload)?;
    link.answer(what, CONTROL_TIMEOUT).map(drop)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::{TcpListener, TcpStream},
    };

    use sdrmm_device::net::Endpoint;

    use super::*;
    use crate::iio::net::NetTransport;

    /// Answers each command with a scripted reply, which is exactly the shape iiod speaks in.
    fn scripted(replies: Vec<Vec<u8>>) -> (Client, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut seen = Vec::new();
            for reply in replies {
                seen.push(read_command(&mut stream));
                if stream.write_all(&reply).is_err() {
                    break;
                }
            }
            let mut rest = Vec::new();
            let _ = stream.read_to_end(&mut rest);
            seen.push(String::from_utf8_lossy(&rest).to_string());
            seen
        });
        let endpoint = Endpoint::parse(&format!("127.0.0.1:{port}"), 30_431).expect("endpoint");
        let transport = NetTransport::connect(&endpoint).expect("connect");
        (Client::new(Arc::new(transport)), server)
    }

    fn read_command(stream: &mut TcpStream) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read_exact(&mut byte).is_ok() {
            line.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        String::from_utf8_lossy(&line).to_string()
    }

    #[test]
    fn an_attribute_read_takes_the_value_and_drops_its_terminator() {
        let (client, server) = scripted(vec![b"11\n2400000000\n\n".to_vec()]);
        let value = client
            .read_channel_attr("ad9361-phy", Direction::Out, "altvoltage0", "frequency")
            .expect("the radio answered");
        assert_eq!(value, "2400000000");
        client.close();
        let seen = server.join().expect("server");
        assert_eq!(seen[0], "READ ad9361-phy OUTPUT altvoltage0 frequency\r\n");
    }

    #[test]
    fn an_attribute_write_sends_the_value_with_its_terminator_and_is_acknowledged() {
        let (client, server) = scripted(vec![b"11\n".to_vec()]);
        client
            .write_channel_attr(
                "ad9361-phy",
                Direction::In,
                "voltage0",
                "hardwaregain",
                "40.000000",
            )
            .expect("accepted");
        client.close();
        let seen = server.join().expect("server");
        assert_eq!(
            seen[0], "WRITE ad9361-phy INPUT voltage0 hardwaregain 10\r\n",
            "the byte count includes the terminator"
        );
        assert!(seen[1].starts_with("40.000000\0"), "{:?}", seen[1]);
    }

    #[test]
    fn a_refusal_carries_the_attribute_that_was_refused() {
        let (client, server) = scripted(vec![b"-22\n".to_vec()]);
        let error = client
            .write_device_attr("ad9361-phy", "ensm_mode", "nonsense")
            .expect_err("refused");
        assert!(matches!(error, DeviceError::Unsupported(_)));
        assert!(error.to_string().contains("ensm_mode"), "{error}");
        client.close();
        let _ = server.join();
    }

    #[test]
    fn a_context_description_is_parsed_out_of_the_print_answer() {
        let xml = "<context name=\"n\" ><device id=\"iio:device0\" name=\"ad9361-phy\" >\
                   </device></context>";
        let reply = format!("{}\n{xml}\n", xml.len()).into_bytes();
        let (client, server) = scripted(vec![reply]);
        let context = client.context().expect("a context");
        assert!(context.device("ad9361-phy").is_some());
        client.close();
        assert_eq!(server.join().expect("server")[0], "PRINT\r\n");
    }

    #[test]
    fn an_answer_that_is_not_a_number_is_reported_rather_than_read_as_data() {
        let (client, server) = scripted(vec![b"weird\n".to_vec()]);
        let error = client
            .read_device_attr("ad9361-phy", "ensm_mode")
            .expect_err("refused");
        assert!(error.to_string().contains("weird"), "{error}");
        client.close();
        let _ = server.join();
    }

    #[test]
    fn a_value_larger_than_any_attribute_is_skipped_rather_than_allocated() {
        let (client, server) = scripted(vec![format!("{}\n", MAX_ATTR + 1).into_bytes()]);
        let error = client
            .read_device_attr("ad9361-phy", "filter_fir_config")
            .expect_err("refused");
        assert!(error.to_string().contains("is not a value"), "{error}");
        client.close();
        let _ = server.join();
    }
}

use crate::DeviceError;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) const TERMINATOR: &[u8; 4] = b"\r\n\r\n";

pub(crate) const MAX_RESPONSE: usize = 16 << 10;

pub(crate) fn nonce() -> String {
    let mut bytes = [0u8; 16];
    for (half, chunk) in bytes.chunks_mut(8).enumerate() {
        let source = super::frame::entropy() ^ (half as u64).rotate_left(32);
        chunk.copy_from_slice(&source.to_le_bytes());
    }
    base64(&bytes)
}

pub(crate) fn request(host: &str, path: &str, key: &str) -> Vec<u8> {
    format!(
        "GET {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    )
    .into_bytes()
}

pub(crate) fn accept(key: &str) -> String {
    base64(&sha1(format!("{key}{GUID}").as_bytes()))
}

/// Checks the upgrade the server answered with, so a plain HTTP page or a redirect is reported as
/// what it is instead of being read as frames.
pub(crate) fn verify(response: &[u8], key: &str) -> Result<(), DeviceError> {
    let text = String::from_utf8_lossy(response);
    let mut lines = text.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code = status.split_whitespace().nth(1).unwrap_or_default();
    if code != "101" {
        return Err(DeviceError::Io(format!(
            "the server answered the WebSocket upgrade with {}",
            status.trim()
        )));
    }
    let header = |wanted: &str| {
        lines.clone().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| value.trim())
        })
    };
    if !header("upgrade").is_some_and(|value| value.eq_ignore_ascii_case("websocket")) {
        return Err(DeviceError::Io(
            "the server accepted the upgrade without switching to websocket".to_string(),
        ));
    }
    let upgrading = |value: &str| {
        value
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    };
    if !header("connection").is_some_and(upgrading) {
        return Err(DeviceError::Io(
            "the server's switch carries no Connection: Upgrade".to_string(),
        ));
    }
    let expected = accept(key);
    match header("sec-websocket-accept") {
        Some(value) if value == expected => Ok(()),
        Some(_) => Err(DeviceError::Io(
            "the server's Sec-WebSocket-Accept does not answer the key SDR-- sent".to_string(),
        )),
        None => Err(DeviceError::Io(
            "the server's upgrade carries no Sec-WebSocket-Accept".to_string(),
        )),
    }
}

pub(crate) fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (at, byte)| {
            word | u32::from(*byte) << (16 - at * 8)
        });
        for at in 0..=chunk.len() {
            let index = (word >> (18 - at * 6)) & 0x3F;
            out.push(char::from(ALPHABET[index as usize]));
        }
        for _ in chunk.len()..3 {
            out.push('=');
        }
    }
    out
}

pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());

    let (blocks, _) = message.as_chunks::<64>();
    for block in blocks {
        let mut words = [0u32; 80];
        let (chunks, _) = block.as_chunks::<4>();
        for (word, bytes) in words.iter_mut().zip(chunks) {
            *word = u32::from_be_bytes(*bytes);
        }
        for at in 16..80 {
            words[at] =
                (words[at - 3] ^ words[at - 8] ^ words[at - 14] ^ words[at - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (at, word) in words.into_iter().enumerate() {
            let (mix, constant) = match at {
                0..20 => ((b & c) | (!b & d), 0x5A82_7999),
                20..40 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(mix)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 20];
    for (bytes, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *bytes = word.to_be_bytes();
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(digest: [u8; 20]) -> String {
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha1_matches_the_published_vectors() {
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            "a message spanning two blocks"
        );
        assert_eq!(
            hex(sha1(&[b'a'; 1000])),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    #[test]
    fn base64_pads_the_way_the_alphabet_says() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xEF, 0xFE]), "/+/+", "the last two symbols");
    }

    #[test]
    fn the_accept_answers_the_key_from_rfc_6455() {
        assert_eq!(
            accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn a_nonce_is_sixteen_bytes_of_base64_and_not_the_same_one_twice() {
        let first = nonce();
        assert_eq!(first.len(), 24);
        assert!(first.ends_with("=="));
        let keys: std::collections::BTreeSet<String> = (0..16).map(|_| nonce()).collect();
        assert!(keys.len() > 1, "a constant key is not a key");
    }

    #[test]
    fn the_request_names_the_host_the_path_and_the_key() {
        let request = String::from_utf8_lossy(&request("radio.local:5454", "/", "KEY")).to_string();
        assert!(request.starts_with("GET / HTTP/1.1\r\n"));
        assert!(request.contains("Host: radio.local:5454\r\n"));
        assert!(request.contains("Sec-WebSocket-Key: KEY\r\n"));
        assert!(request.contains("Sec-WebSocket-Version: 13\r\n"));
        assert!(request.ends_with("\r\n\r\n"));
    }

    fn upgrade(key: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept(key)
        )
        .into_bytes()
    }

    #[test]
    fn a_real_upgrade_is_accepted_however_the_headers_are_cased() {
        assert!(
            verify(
                &upgrade("dGhlIHNhbXBsZSBub25jZQ=="),
                "dGhlIHNhbXBsZSBub25jZQ=="
            )
            .is_ok()
        );
    }

    #[test]
    fn anything_that_is_not_an_upgrade_is_refused_by_name() {
        let plain = b"HTTP/1.1 404 Not Found\r\n\r\n";
        assert!(
            verify(plain, "KEY").is_err_and(|e| e.to_string().contains("404")),
            "a web server is not a WebSocket server"
        );
        let no_accept =
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n";
        assert!(verify(no_accept, "KEY").is_err_and(|e| e.to_string().contains("Accept")));
        let wrong = verify(&upgrade("other"), "KEY");
        assert!(wrong.is_err_and(|e| e.to_string().contains("does not answer")));
        let not_websocket =
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: h2c\r\nSec-WebSocket-Accept: x\r\n\r\n";
        assert!(verify(not_websocket, "KEY").is_err_and(|e| e.to_string().contains("websocket")));
    }

    #[test]
    fn a_switch_that_does_not_connect_upgrade_is_refused() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let without = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept(key)
        );
        assert!(
            verify(without.as_bytes(), key).is_err_and(|e| e.to_string().contains("Connection")),
            "RFC 6455 4.1 makes the client reject a 101 without the Upgrade token"
        );
        let keep_alive = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept(key)
        );
        assert!(
            verify(keep_alive.as_bytes(), key).is_err_and(|e| e.to_string().contains("Connection"))
        );
    }

    #[test]
    fn a_connection_header_listing_more_than_upgrade_still_counts() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let listed = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive, UPGRADE\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept(key)
        );
        assert!(verify(listed.as_bytes(), key).is_ok());
    }
}

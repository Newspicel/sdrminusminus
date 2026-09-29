use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

pub const PROTOCOL: &str = "sdrmm-tunnel.1";
pub const WINDOW: u32 = 256 * 1024;
pub const CONTROL_STREAM: u32 = 0;
pub const NONCE_LEN: usize = 32;
pub const PROOF_LEN: usize = 64;

const HEADER_LEN: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestHead {
    pub method: String,
    pub uri: String,
    pub headers: Vec<(String, String)>,
    pub websocket: bool,
    pub user: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    Challenge {
        nonce: [u8; NONCE_LEN],
    },
    Proof {
        signature: [u8; PROOF_LEN],
    },
    Ready,
    Request {
        stream: u32,
        head: RequestHead,
    },
    Response {
        stream: u32,
        head: ResponseHead,
    },
    Body {
        stream: u32,
        data: Bytes,
    },
    End {
        stream: u32,
    },
    Text {
        stream: u32,
        data: Bytes,
    },
    Binary {
        stream: u32,
        data: Bytes,
    },
    Close {
        stream: u32,
        code: u16,
        reason: String,
    },
    Reset {
        stream: u32,
        reason: String,
    },
    Credit {
        stream: u32,
        bytes: u32,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame shorter than its {HEADER_LEN}-byte header")]
    Truncated,
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    #[error("{kind} payload must be {expected} bytes, got {got}")]
    Length {
        kind: &'static str,
        expected: usize,
        got: usize,
    },
    #[error("{0} carries text that is not UTF-8")]
    NotUtf8(&'static str),
    #[error("{kind} head: {detail}")]
    Head { kind: &'static str, detail: String },
}

mod kind {
    pub const CHALLENGE: u8 = 1;
    pub const PROOF: u8 = 2;
    pub const READY: u8 = 3;
    pub const REQUEST: u8 = 4;
    pub const RESPONSE: u8 = 5;
    pub const BODY: u8 = 6;
    pub const END: u8 = 7;
    pub const TEXT: u8 = 8;
    pub const BINARY: u8 = 9;
    pub const CLOSE: u8 = 10;
    pub const RESET: u8 = 11;
    pub const CREDIT: u8 = 12;
}

impl Frame {
    pub fn stream(&self) -> u32 {
        match self {
            Self::Challenge { .. } | Self::Proof { .. } | Self::Ready => CONTROL_STREAM,
            Self::Request { stream, .. }
            | Self::Response { stream, .. }
            | Self::Body { stream, .. }
            | Self::End { stream }
            | Self::Text { stream, .. }
            | Self::Binary { stream, .. }
            | Self::Close { stream, .. }
            | Self::Reset { stream, .. }
            | Self::Credit { stream, .. } => *stream,
        }
    }

    pub fn credited_len(&self) -> usize {
        match self {
            Self::Body { data, .. } | Self::Text { data, .. } | Self::Binary { data, .. } => {
                data.len()
            }
            _ => 0,
        }
    }

    pub fn encode(&self) -> Result<Bytes, FrameError> {
        let (kind, payload) = match self {
            Self::Challenge { nonce } => (kind::CHALLENGE, Bytes::copy_from_slice(nonce)),
            Self::Proof { signature } => (kind::PROOF, Bytes::copy_from_slice(signature)),
            Self::Ready => (kind::READY, Bytes::new()),
            Self::Request { head, .. } => (kind::REQUEST, json("request", head)?),
            Self::Response { head, .. } => (kind::RESPONSE, json("response", head)?),
            Self::Body { data, .. } => (kind::BODY, data.clone()),
            Self::End { .. } => (kind::END, Bytes::new()),
            Self::Text { data, .. } => (kind::TEXT, data.clone()),
            Self::Binary { data, .. } => (kind::BINARY, data.clone()),
            Self::Close { code, reason, .. } => {
                let mut payload = BytesMut::with_capacity(2 + reason.len());
                payload.put_u16(*code);
                payload.put_slice(reason.as_bytes());
                (kind::CLOSE, payload.freeze())
            }
            Self::Reset { reason, .. } => (kind::RESET, Bytes::copy_from_slice(reason.as_bytes())),
            Self::Credit { bytes, .. } => {
                (kind::CREDIT, Bytes::copy_from_slice(&bytes.to_be_bytes()))
            }
        };
        let mut out = BytesMut::with_capacity(HEADER_LEN + payload.len());
        out.put_u8(kind);
        out.put_u32(self.stream());
        out.put_slice(&payload);
        Ok(out.freeze())
    }

    pub fn decode(mut bytes: Bytes) -> Result<Self, FrameError> {
        if bytes.len() < HEADER_LEN {
            return Err(FrameError::Truncated);
        }
        let kind = bytes.get_u8();
        let stream = bytes.get_u32();
        let payload = bytes;
        Ok(match kind {
            kind::CHALLENGE => Self::Challenge {
                nonce: fixed("challenge", &payload)?,
            },
            kind::PROOF => Self::Proof {
                signature: fixed("proof", &payload)?,
            },
            kind::READY => Self::Ready,
            kind::REQUEST => Self::Request {
                stream,
                head: parse("request", &payload)?,
            },
            kind::RESPONSE => Self::Response {
                stream,
                head: parse("response", &payload)?,
            },
            kind::BODY => Self::Body {
                stream,
                data: payload,
            },
            kind::END => Self::End { stream },
            kind::TEXT => {
                std::str::from_utf8(&payload).map_err(|_| FrameError::NotUtf8("text"))?;
                Self::Text {
                    stream,
                    data: payload,
                }
            }
            kind::BINARY => Self::Binary {
                stream,
                data: payload,
            },
            kind::CLOSE => close(stream, payload)?,
            kind::RESET => Self::Reset {
                stream,
                reason: utf8("reset", payload)?,
            },
            kind::CREDIT => Self::Credit {
                stream,
                bytes: u32::from_be_bytes(fixed("credit", &payload)?),
            },
            other => return Err(FrameError::UnknownKind(other)),
        })
    }
}

fn close(stream: u32, mut payload: Bytes) -> Result<Frame, FrameError> {
    if payload.len() < 2 {
        return Err(FrameError::Length {
            kind: "close",
            expected: 2,
            got: payload.len(),
        });
    }
    let code = payload.get_u16();
    Ok(Frame::Close {
        stream,
        code,
        reason: utf8("close", payload)?,
    })
}

fn fixed<const N: usize>(kind: &'static str, payload: &[u8]) -> Result<[u8; N], FrameError> {
    payload.try_into().map_err(|_| FrameError::Length {
        kind,
        expected: N,
        got: payload.len(),
    })
}

fn utf8(kind: &'static str, payload: Bytes) -> Result<String, FrameError> {
    String::from_utf8(payload.to_vec()).map_err(|_| FrameError::NotUtf8(kind))
}

fn json<T: Serialize>(kind: &'static str, head: &T) -> Result<Bytes, FrameError> {
    serde_json::to_vec(head)
        .map(Bytes::from)
        .map_err(|error| FrameError::Head {
            kind,
            detail: error.to_string(),
        })
}

fn parse<T: for<'de> Deserialize<'de>>(
    kind: &'static str,
    payload: &[u8],
) -> Result<T, FrameError> {
    serde_json::from_slice(payload).map_err(|error| FrameError::Head {
        kind,
        detail: error.to_string(),
    })
}

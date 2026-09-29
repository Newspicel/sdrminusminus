#![allow(clippy::expect_used)]
use bytes::Bytes;
use sdrmm_tunnel::{
    DeviceKey,
    frame::{Frame, FrameError, RequestHead, ResponseHead},
    identity::verify_proof,
};
use serde_json::{Value, json};

const VECTORS: &str = include_str!("../vectors.json");

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn describe(frame: &Frame) -> Value {
    let stream = frame.stream();
    match frame {
        Frame::Challenge { nonce } => {
            json!({"kind": "challenge", "stream": stream, "nonce": hex(nonce)})
        }
        Frame::Proof { signature } => {
            json!({"kind": "proof", "stream": stream, "signature": hex(signature)})
        }
        Frame::Ready => json!({"kind": "ready", "stream": stream}),
        Frame::Request { head, .. } => json!({"kind": "request", "stream": stream, "head": head}),
        Frame::Response { head, .. } => json!({"kind": "response", "stream": stream, "head": head}),
        Frame::Body { data, .. } => json!({"kind": "body", "stream": stream, "data": hex(data)}),
        Frame::End { .. } => json!({"kind": "end", "stream": stream}),
        Frame::Text { data, .. } => json!({"kind": "text", "stream": stream, "data": hex(data)}),
        Frame::Binary { data, .. } => {
            json!({"kind": "binary", "stream": stream, "data": hex(data)})
        }
        Frame::Close { code, reason, .. } => {
            json!({"kind": "close", "stream": stream, "code": code, "reason": reason})
        }
        Frame::Reset { reason, .. } => json!({"kind": "reset", "stream": stream, "reason": reason}),
        Frame::Credit { bytes, .. } => json!({"kind": "credit", "stream": stream, "bytes": bytes}),
        Frame::Health { data } => json!({"kind": "health", "stream": stream, "data": hex(data)}),
    }
}

fn every_kind() -> Vec<Frame> {
    vec![
        Frame::Challenge { nonce: [0xa5; 32] },
        Frame::Proof {
            signature: [0x5a; 64],
        },
        Frame::Ready,
        Frame::Request {
            stream: 1,
            head: RequestHead {
                method: "GET".to_string(),
                uri: "/api/state?x=1".to_string(),
                headers: vec![
                    ("host".to_string(), "abc.sdrmm.link".to_string()),
                    (
                        "content-disposition".to_string(),
                        "Gr\u{fc}\u{df}e".to_string(),
                    ),
                ],
                websocket: false,
                user: "user-1".to_string(),
            },
        },
        Frame::Response {
            stream: 1,
            head: ResponseHead {
                status: 200,
                headers: vec![("content-type".to_string(), "text/plain".to_string())],
            },
        },
        Frame::Body {
            stream: 1,
            data: Bytes::from_static(b"hi"),
        },
        Frame::End { stream: 1 },
        Frame::Text {
            stream: 0x0102_0304,
            data: Bytes::from_static("\u{fc}".as_bytes()),
        },
        Frame::Binary {
            stream: 2,
            data: Bytes::from_static(&[0, 255]),
        },
        Frame::Close {
            stream: 2,
            code: 1000,
            reason: "bye".to_string(),
        },
        Frame::Reset {
            stream: 3,
            reason: "gone".to_string(),
        },
        Frame::Credit {
            stream: 3,
            bytes: 65_536,
        },
        Frame::Health {
            data: Bytes::from_static(br#"{"rtt_ms":null,"site":{}}"#),
        },
    ]
}

#[test]
fn every_frame_round_trips() {
    for frame in every_kind() {
        let bytes = frame.encode().expect("encode");
        assert_eq!(Frame::decode(bytes).expect("decode"), frame);
    }
}

#[test]
fn the_shared_vectors_match_the_codec() {
    let expected: Vec<Value> = every_kind()
        .iter()
        .map(|frame| {
            json!({
                "hex": hex(&frame.encode().expect("encode")),
                "frame": describe(frame),
            })
        })
        .collect();
    let stored: Vec<Value> = serde_json::from_str(VECTORS).expect("vectors.json");
    assert_eq!(
        stored,
        expected,
        "vectors.json is stale; expected:\n{}",
        serde_json::to_string_pretty(&expected).expect("json")
    );
    for vector in &stored {
        let bytes = unhex(vector["hex"].as_str().expect("hex field"));
        let frame = Frame::decode(Bytes::from(bytes)).expect("decode vector");
        assert_eq!(describe(&frame), vector["frame"]);
    }
}

#[test]
fn malformed_frames_are_rejected() {
    assert_eq!(
        Frame::decode(Bytes::from_static(&[6, 0, 0])),
        Err(FrameError::Truncated)
    );
    assert_eq!(
        Frame::decode(Bytes::from_static(&[99, 0, 0, 0, 1])),
        Err(FrameError::UnknownKind(99))
    );
    assert!(matches!(
        Frame::decode(Bytes::from_static(&[1, 0, 0, 0, 0, 1, 2])),
        Err(FrameError::Length {
            kind: "challenge",
            ..
        })
    ));
    assert_eq!(
        Frame::decode(Bytes::from_static(&[8, 0, 0, 0, 1, 0xff])),
        Err(FrameError::NotUtf8("text"))
    );
    assert_eq!(
        Frame::decode(Bytes::from_static(&[13, 0, 0, 0, 0, b'{', 0xff])),
        Err(FrameError::NotUtf8("health"))
    );
    assert!(matches!(
        Frame::decode(Bytes::from_static(&[4, 0, 0, 0, 1, b'{'])),
        Err(FrameError::Head {
            kind: "request",
            ..
        })
    ));
}

#[test]
fn a_proof_verifies_only_for_its_key_and_nonce() {
    let (key, document) = DeviceKey::generate().expect("key");
    let public_key = key.public_key().expect("public key");
    let nonce = [1u8; 32];
    let proof = key.prove(&nonce).expect("proof");
    assert!(verify_proof(&public_key, &nonce, &proof));
    assert!(!verify_proof(&public_key, &[2u8; 32], &proof));
    let (other, _) = DeviceKey::generate().expect("other key");
    assert!(!verify_proof(
        &other.public_key().expect("other public"),
        &nonce,
        &proof
    ));
    let reloaded = DeviceKey::from_pkcs8(&document).expect("reload");
    assert_eq!(reloaded.public_key().expect("reloaded public"), public_key);
    assert!(DeviceKey::from_pkcs8(b"not a key").is_err());
}

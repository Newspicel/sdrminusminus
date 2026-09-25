use std::time::Duration;

use futures::{SinkExt, StreamExt};
use sdrmm_wire::{
    frame::{FrameHeader, FrameKind, SpectrumFrame},
    ws::{ClientCommand, ServerEvent},
};
use tokio::sync::mpsc;

use crate::{api::Token, bus::Frame};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
pub struct Spectrum {
    pub stream_id: u16,
    pub seq: u32,
    pub center_hz: f64,
    pub span_hz: f32,
    pub db_min: f32,
    pub db_max: f32,
    pub bins: Vec<u8>,
}

#[derive(Debug)]
pub enum Incoming {
    Up,
    Down,
    Event(Box<ServerEvent>),
    Frame(Frame),
}

#[derive(Clone)]
pub struct Socket {
    commands: mpsc::UnboundedSender<ClientCommand>,
}

impl Socket {
    pub fn send(&self, command: ClientCommand) {
        if self.commands.send(command).is_err() {
            tracing::debug!("the socket is gone; command dropped");
        }
    }
}

pub fn connect(
    runtime: &tokio::runtime::Handle,
    url: String,
    token: Token,
) -> (Socket, mpsc::UnboundedReceiver<Incoming>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
    runtime.spawn(run(url, token, command_rx, incoming_tx));
    (
        Socket {
            commands: command_tx,
        },
        incoming_rx,
    )
}

#[must_use]
pub fn with_token(url: &str, token: Option<&str>) -> String {
    match token.filter(|token| !token.is_empty()) {
        Some(token) => {
            let mut parsed = match url::Url::parse(url) {
                Ok(parsed) => parsed,
                Err(_) => return url.to_owned(),
            };
            parsed.query_pairs_mut().append_pair("token", token);
            parsed.to_string()
        }
        None => url.to_owned(),
    }
}

async fn run(
    url: String,
    token: Token,
    mut commands: mpsc::UnboundedReceiver<ClientCommand>,
    incoming: mpsc::UnboundedSender<Incoming>,
) {
    let mut backoff = Duration::from_millis(250);
    loop {
        let address = with_token(&url, token.get().as_deref());
        match tokio_tungstenite::connect_async(&address).await {
            Ok((stream, _)) => {
                backoff = Duration::from_millis(250);
                if incoming.send(Incoming::Up).is_err() {
                    return;
                }
                let closed = pump(stream, &mut commands, &incoming).await;
                if incoming.send(Incoming::Down).is_err() || closed {
                    return;
                }
            }
            Err(error) => {
                tracing::debug!(%error, "the socket did not open");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
            }
        }
    }
}

async fn pump<S>(
    stream: tokio_tungstenite::WebSocketStream<S>,
    commands: &mut mpsc::UnboundedReceiver<ClientCommand>,
    incoming: &mpsc::UnboundedSender<Incoming>,
) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut source) = stream.split();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return true };
                if write(&mut sink, &command).await.is_err() {
                    return false;
                }
            }
            message = source.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                        Ok(event) => {
                            if incoming.send(Incoming::Event(Box::new(event))).is_err() {
                                return true;
                            }
                        }
                        Err(error) => tracing::debug!(%error, "unreadable server event"),
                    },
                    Some(Ok(Message::Binary(bytes))) => {
                        match Frame::read(&bytes) {
                            Some(frame) => {
                                if incoming.send(Incoming::Frame(frame)).is_err() {
                                    return true;
                                }
                            }
                            None => tracing::debug!(len = bytes.len(), "unreadable frame"),
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        tracing::debug!(%error, "the socket failed");
                        return false;
                    }
                    None => return false,
                }
            }
        }
    }
}

async fn write<S>(
    sink: &mut futures::stream::SplitSink<tokio_tungstenite::WebSocketStream<S>, Message>,
    command: &ClientCommand,
) -> Result<(), ()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let text = serde_json::to_string(command).map_err(|error| {
        tracing::error!(%error, "a command would not serialise");
    })?;
    sink.send(Message::text(text)).await.map_err(|error| {
        tracing::debug!(%error, "a command did not reach the server");
    })
}

pub fn spectrum(bytes: &[u8]) -> Option<Spectrum> {
    let header = FrameHeader::parse(bytes)?;
    if header.kind != FrameKind::Spectrum {
        return None;
    }
    let frame = SpectrumFrame::decode(bytes)?;
    Some(Spectrum {
        stream_id: frame.stream_id,
        seq: frame.seq,
        center_hz: frame.center_hz,
        span_hz: frame.span_hz,
        db_min: frame.db_min,
        db_max: frame.db_max,
        bins: frame.bins.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spectrum_frame_is_read_off_the_wire_and_another_kind_is_not() {
        let bins: Vec<u8> = (0..128u8).collect();
        let encoded = SpectrumFrame {
            stream_id: 5,
            seq: 1,
            timestamp: 0,
            center_hz: 100e6,
            span_hz: 2.048e6,
            db_min: -120.0,
            db_max: -20.0,
            bins: &bins,
        }
        .encode();
        let read = spectrum(&encoded).expect("a spectrum");
        assert_eq!(read.stream_id, 5);
        assert_eq!(read.seq, 1);
        assert_eq!(read.bins, bins);
        assert_eq!(read.center_hz, 100e6);

        let audio = sdrmm_wire::frame::AudioFrame {
            stream_id: 5,
            seq: 1,
            timestamp: 0,
            ch_layout: 1,
            opus: &[1, 2, 3],
        }
        .encode();
        assert!(spectrum(&audio).is_none());
        assert!(spectrum(&[]).is_none());
    }

    #[test]
    fn a_held_token_rides_on_the_socket_address() {
        assert_eq!(with_token("ws://h/api/ws", None), "ws://h/api/ws");
        assert_eq!(with_token("ws://h/api/ws", Some("")), "ws://h/api/ws");
        assert_eq!(
            with_token("ws://h/api/ws", Some("a b")),
            "ws://h/api/ws?token=a+b"
        );
    }
}

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use sdrmm_wire::ws::{ClientCommand, ServerEvent};
use tokio::sync::mpsc;

use crate::bus::Frame;
use tokio_tungstenite::tungstenite::Message;

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
) -> (Socket, mpsc::UnboundedReceiver<Incoming>) {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
    runtime.spawn(run(url, command_rx, incoming_tx));
    (
        Socket {
            commands: command_tx,
        },
        incoming_rx,
    )
}

async fn run(
    url: String,
    mut commands: mpsc::UnboundedReceiver<ClientCommand>,
    incoming: mpsc::UnboundedSender<Incoming>,
) {
    let mut backoff = Duration::from_millis(250);
    loop {
        match tokio_tungstenite::connect_async(&url).await {
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

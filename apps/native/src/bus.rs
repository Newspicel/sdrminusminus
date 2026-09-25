use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use sdrmm_wire::{
    frame::{FrameHeader, FrameKind},
    ws::{ClientCommand, ServerEvent},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Spectrum { device_set: u32, stream: u32 },
    Audio { device_set: u32, channel: u32, fx: Vec<String> },
    Video { device_set: u32, channel: u32 },
    Iq { device_set: u32, channel: u32 },
    Symbols { device_set: u32, channel: u32 },
    Surface { device_set: u32, node: String },
}

impl Source {
    #[must_use]
    pub fn started(event: &ServerEvent) -> Option<(u16, Self)> {
        match event {
            ServerEvent::StreamStarted {
                stream_id,
                device_set,
                stream,
            } => Some((
                *stream_id,
                Self::Spectrum {
                    device_set: *device_set,
                    stream: *stream,
                },
            )),
            ServerEvent::AudioStreamStarted {
                stream_id,
                device_set,
                channel,
                fx,
            } => Some((
                *stream_id,
                Self::Audio {
                    device_set: *device_set,
                    channel: *channel,
                    fx: fx.clone(),
                },
            )),
            ServerEvent::VideoStreamStarted {
                stream_id,
                device_set,
                channel,
            } => Some((
                *stream_id,
                Self::Video {
                    device_set: *device_set,
                    channel: *channel,
                },
            )),
            ServerEvent::IqStreamStarted {
                stream_id,
                device_set,
                channel,
            } => Some((
                *stream_id,
                Self::Iq {
                    device_set: *device_set,
                    channel: *channel,
                },
            )),
            ServerEvent::SymbolStreamStarted {
                stream_id,
                device_set,
                channel,
            } => Some((
                *stream_id,
                Self::Symbols {
                    device_set: *device_set,
                    channel: *channel,
                },
            )),
            ServerEvent::SurfaceStreamStarted {
                stream_id,
                device_set,
                node,
            } => Some((
                *stream_id,
                Self::Surface {
                    device_set: *device_set,
                    node: node.clone(),
                },
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub kind: FrameKind,
    pub stream_id: u16,
    pub bytes: Arc<[u8]>,
}

impl Frame {
    #[must_use]
    pub fn read(bytes: &[u8]) -> Option<Self> {
        let header = FrameHeader::parse(bytes)?;
        Some(Self {
            kind: header.kind,
            stream_id: header.stream_id,
            bytes: Arc::from(bytes),
        })
    }
}

#[must_use]
pub fn subscription_key(command: &ClientCommand) -> Option<(String, bool)> {
    let key = |name: &str, device_set: u32, rest: String| format!("{name}/{device_set}/{rest}");
    Some(match command {
        ClientCommand::SubscribeSpectrum {
            device_set, stream, ..
        } => (key("spectrum", *device_set, stream.to_string()), true),
        ClientCommand::UnsubscribeSpectrum { device_set, stream } => {
            (key("spectrum", *device_set, stream.to_string()), false)
        }
        ClientCommand::SubscribeAudio {
            device_set,
            channel,
            fx,
        } => (key("audio", *device_set, format!("{channel}/{}", fx.join(","))), true),
        ClientCommand::UnsubscribeAudio {
            device_set,
            channel,
            fx,
        } => (key("audio", *device_set, format!("{channel}/{}", fx.join(","))), false),
        ClientCommand::SubscribeVideo {
            device_set,
            channel,
        } => (key("video", *device_set, channel.to_string()), true),
        ClientCommand::UnsubscribeVideo {
            device_set,
            channel,
        } => (key("video", *device_set, channel.to_string()), false),
        ClientCommand::SubscribeIq {
            device_set,
            channel,
        } => (key("iq", *device_set, channel.to_string()), true),
        ClientCommand::UnsubscribeIq {
            device_set,
            channel,
        } => (key("iq", *device_set, channel.to_string()), false),
        ClientCommand::SubscribeSymbols {
            device_set,
            channel,
        } => (key("symbols", *device_set, channel.to_string()), true),
        ClientCommand::UnsubscribeSymbols {
            device_set,
            channel,
        } => (key("symbols", *device_set, channel.to_string()), false),
        ClientCommand::SubscribeSurface { node } => (format!("surface/{node}"), true),
        ClientCommand::UnsubscribeSurface { node } => (format!("surface/{node}"), false),
        ClientCommand::SubscribeDiagnostics { enabled } => (String::from("diagnostics"), *enabled),
        ClientCommand::PublishPosition { .. } => return None,
    })
}

#[must_use]
pub fn unsubscribe_of(command: &ClientCommand) -> Option<ClientCommand> {
    Some(match command.clone() {
        ClientCommand::SubscribeSpectrum {
            device_set, stream, ..
        } => ClientCommand::UnsubscribeSpectrum { device_set, stream },
        ClientCommand::SubscribeAudio {
            device_set,
            channel,
            fx,
        } => ClientCommand::UnsubscribeAudio {
            device_set,
            channel,
            fx,
        },
        ClientCommand::SubscribeVideo {
            device_set,
            channel,
        } => ClientCommand::UnsubscribeVideo {
            device_set,
            channel,
        },
        ClientCommand::SubscribeIq {
            device_set,
            channel,
        } => ClientCommand::UnsubscribeIq {
            device_set,
            channel,
        },
        ClientCommand::SubscribeSymbols {
            device_set,
            channel,
        } => ClientCommand::UnsubscribeSymbols {
            device_set,
            channel,
        },
        ClientCommand::SubscribeSurface { node } => ClientCommand::UnsubscribeSurface { node },
        ClientCommand::SubscribeDiagnostics { enabled: true } => {
            ClientCommand::SubscribeDiagnostics { enabled: false }
        }
        _ => return None,
    })
}

type Handler<T> = Rc<dyn Fn(&T)>;

pub struct Listeners<T> {
    next: u64,
    held: Vec<(u64, Handler<T>)>,
}

impl<T> Default for Listeners<T> {
    fn default() -> Self {
        Self {
            next: 0,
            held: Vec::new(),
        }
    }
}

impl<T> Listeners<T> {
    pub fn add(&mut self, handler: impl Fn(&T) + 'static) -> u64 {
        self.next += 1;
        self.held.push((self.next, Rc::new(handler)));
        self.next
    }

    pub fn remove(&mut self, id: u64) {
        self.held.retain(|(held, _)| *held != id);
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<Handler<T>> {
        self.held.iter().map(|(_, handler)| handler.clone()).collect()
    }
}

#[derive(Default)]
pub struct Bus {
    pub events: RefCell<Listeners<ServerEvent>>,
    pub frames: RefCell<Listeners<Frame>>,
    pub sources: RefCell<HashMap<u16, Source>>,
    pub holds: RefCell<HashMap<String, (ClientCommand, usize)>>,
}

impl Bus {
    pub fn publish_event(&self, event: &ServerEvent) {
        if let Some((stream_id, source)) = Source::started(event) {
            self.sources.borrow_mut().insert(stream_id, source);
        }
        if let ServerEvent::StreamStopped { stream_id, .. } = event {
            self.sources.borrow_mut().remove(stream_id);
        }
        let handlers = self.events.borrow().snapshot();
        for handler in handlers {
            handler(event);
        }
    }

    pub fn publish_frame(&self, frame: &Frame) {
        let handlers = self.frames.borrow().snapshot();
        for handler in handlers {
            handler(frame);
        }
    }

    #[must_use]
    pub fn source_of(&self, stream_id: u16) -> Option<Source> {
        self.sources.borrow().get(&stream_id).cloned()
    }

    pub fn hold(&self, command: &ClientCommand) -> Option<ClientCommand> {
        let (key, _) = subscription_key(command)?;
        let mut holds = self.holds.borrow_mut();
        let entry = holds.entry(key).or_insert_with(|| (command.clone(), 0));
        entry.1 += 1;
        (entry.1 == 1).then(|| command.clone())
    }

    pub fn release(&self, command: &ClientCommand) -> Option<ClientCommand> {
        let (key, _) = subscription_key(command)?;
        let mut holds = self.holds.borrow_mut();
        let entry = holds.get_mut(&key)?;
        entry.1 = entry.1.saturating_sub(1);
        if entry.1 > 0 {
            return None;
        }
        holds.remove(&key);
        unsubscribe_of(command)
    }

    #[must_use]
    pub fn held(&self) -> Vec<ClientCommand> {
        self.holds
            .borrow()
            .values()
            .map(|(command, _)| command.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use sdrmm_wire::ws::StreamKind;

    use super::*;

    fn audio(channel: u32) -> ClientCommand {
        ClientCommand::SubscribeAudio {
            device_set: 1,
            channel,
            fx: Vec::new(),
        }
    }

    #[test]
    fn two_holders_of_one_stream_subscribe_once_and_release_once() {
        let bus = Bus::default();
        assert!(bus.hold(&audio(0)).is_some());
        assert!(bus.hold(&audio(0)).is_none());
        assert!(bus.release(&audio(0)).is_none());
        assert!(matches!(
            bus.release(&audio(0)),
            Some(ClientCommand::UnsubscribeAudio { channel: 0, .. })
        ));
        assert!(bus.held().is_empty());
    }

    #[test]
    fn every_subscription_has_its_unsubscription() {
        let commands = [
            ClientCommand::SubscribeSpectrum {
                device_set: 1,
                fps: 20,
                bins: 512,
                stream: 0,
            },
            audio(2),
            ClientCommand::SubscribeVideo {
                device_set: 1,
                channel: 0,
            },
            ClientCommand::SubscribeIq {
                device_set: 1,
                channel: 0,
            },
            ClientCommand::SubscribeSymbols {
                device_set: 1,
                channel: 0,
            },
            ClientCommand::SubscribeSurface {
                node: String::from("radar"),
            },
        ];
        for command in commands {
            let off = unsubscribe_of(&command).expect("an unsubscription");
            assert_eq!(subscription_key(&command).map(|(key, _)| key), subscription_key(&off).map(|(key, _)| key));
            assert_eq!(subscription_key(&off).map(|(_, on)| on), Some(false));
        }
    }

    #[test]
    fn a_started_stream_is_routed_until_it_stops() {
        let bus = Bus::default();
        bus.publish_event(&ServerEvent::IqStreamStarted {
            stream_id: 9,
            device_set: 2,
            channel: 4,
        });
        assert_eq!(
            bus.source_of(9),
            Some(Source::Iq {
                device_set: 2,
                channel: 4
            })
        );
        bus.publish_event(&ServerEvent::StreamStopped {
            stream_id: 9,
            kind: StreamKind::Iq,
        });
        assert_eq!(bus.source_of(9), None);
    }

    #[test]
    fn a_removed_listener_hears_nothing_more() {
        let bus = Bus::default();
        let heard = Rc::new(Cell::new(0));
        let counter = heard.clone();
        let id = bus
            .events
            .borrow_mut()
            .add(move |_| counter.set(counter.get() + 1));
        bus.publish_event(&ServerEvent::Hello { revision: 1 });
        bus.events.borrow_mut().remove(id);
        bus.publish_event(&ServerEvent::Hello { revision: 2 });
        assert_eq!(heard.get(), 1);
    }

    #[test]
    fn a_frame_is_read_with_its_kind_and_stream() {
        let bytes = sdrmm_wire::frame::AudioFrame {
            stream_id: 7,
            seq: 1,
            timestamp: 0,
            ch_layout: 1,
            opus: &[1, 2, 3],
        }
        .encode();
        let frame = Frame::read(&bytes).expect("a frame");
        assert_eq!(frame.kind, FrameKind::AudioOpus);
        assert_eq!(frame.stream_id, 7);
        assert!(Frame::read(&[]).is_none());
    }
}

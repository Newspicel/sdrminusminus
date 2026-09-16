use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use sdrmm_device::{
    DeviceError,
    net::{CONNECT_TIMEOUT, Incoming, WebSocket},
};

use crate::proto::{
    Address, Command, Event, Property, Tuner, as_bool, as_f64, as_list, as_u64, decode, flag,
};

/// How long the interrogation waits after the last answer before deciding the rest never came.
const SETTLE: Duration = Duration::from_millis(400);

const POLL: Duration = Duration::from_millis(20);

/// What SDRconnect answered about one tuner, read once while the device is being opened.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot(BTreeMap<Property, String>);

impl Snapshot {
    pub(crate) fn text(&self, property: Property) -> Option<&str> {
        self.0
            .get(&property)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn boolean(&self, property: Property) -> Option<bool> {
        self.text(property).and_then(as_bool)
    }

    pub(crate) fn number(&self, property: Property) -> Option<f64> {
        self.text(property).and_then(as_f64)
    }

    pub(crate) fn count(&self, property: Property) -> Option<u64> {
        self.text(property).and_then(as_u64)
    }

    pub(crate) fn list(&self, property: Property) -> Vec<String> {
        self.text(property).map(as_list).unwrap_or_default()
    }

    pub(crate) fn put(&mut self, property: Property, value: impl Into<String>) {
        self.0.insert(property, value.into());
    }
}

pub(crate) fn connect(address: &Address) -> Result<WebSocket, DeviceError> {
    WebSocket::connect(&address.endpoint, crate::proto::PATH)
}

pub(crate) fn send(
    socket: &WebSocket,
    commands: &[Command],
    tuner: Tuner,
) -> Result<(), DeviceError> {
    for command in commands {
        socket.send_text(&command.encode(tuner)?)?;
    }
    Ok(())
}

/// Aims every event and binary message at one tuner, so a dual-tuner receiver's other half does
/// not arrive on a device that is not it.
pub(crate) fn focus(tuner: Tuner) -> Vec<Command> {
    Tuner::ALL
        .into_iter()
        .map(|which| Command::Emit(which.enable(), flag(which == tuner)))
        .collect()
}

/// Asks for every property the API defines and keeps whatever the server answers.
pub(crate) fn interrogate(socket: &WebSocket, tuner: Tuner) -> Result<Snapshot, DeviceError> {
    send(socket, &focus(tuner), tuner)?;
    let asked: Vec<Command> = Property::ALL.into_iter().map(Command::Get).collect();
    send(socket, &asked, tuner)?;

    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut snapshot = Snapshot::default();
    let mut last = Instant::now();
    while Instant::now() < deadline && snapshot.0.len() < Property::ALL.len() {
        let known = snapshot.0.len();
        match socket.next(POLL) {
            Incoming::Text(text) => match decode(&text) {
                Ok(notification)
                    if matches!(
                        notification.event,
                        Event::GetPropertyResponse | Event::PropertyChanged
                    ) =>
                {
                    if let Some(property) = notification.property {
                        snapshot.put(property, notification.value);
                    }
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("SDRconnect sent a message SDR-- skipped: {e}"),
            },
            Incoming::Binary(_) | Incoming::Idle => {}
            Incoming::Ended => {
                return Err(DeviceError::Io(format!(
                    "the SDRconnect handshake: {}",
                    socket.failure().reason
                )));
            }
        }
        if snapshot.0.len() > known {
            last = Instant::now();
        } else if !snapshot.0.is_empty() && last.elapsed() > SETTLE {
            break;
        }
    }
    if snapshot.0.is_empty() {
        return Err(DeviceError::Io(
            "the server upgraded the WebSocket but answered no property; it is not SDRconnect"
                .to_string(),
        ));
    }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_reads_only_values_that_are_there() {
        let mut snapshot = Snapshot::default();
        snapshot.put(Property::DeviceCenterFrequency, "100000000".to_string());
        snapshot.put(Property::DeviceSampleRate, "2000000.0".to_string());
        snapshot.put(Property::CanControl, "true".to_string());
        snapshot.put(Property::ValidAntennas, "Antenna A, Antenna B".to_string());
        snapshot.put(Property::ActiveAntenna, String::new());

        assert_eq!(
            snapshot.count(Property::DeviceCenterFrequency),
            Some(100_000_000)
        );
        assert_eq!(snapshot.number(Property::DeviceSampleRate), Some(2e6));
        assert_eq!(snapshot.boolean(Property::CanControl), Some(true));
        assert_eq!(
            snapshot.list(Property::ValidAntennas),
            vec!["Antenna A", "Antenna B"]
        );
        assert_eq!(
            snapshot.text(Property::ActiveAntenna),
            None,
            "an empty answer is no answer"
        );
        assert_eq!(snapshot.count(Property::LnaState), None);
    }

    #[test]
    fn focusing_switches_the_other_tuner_off_as_it_switches_this_one_on() {
        assert_eq!(
            focus(Tuner::Secondary),
            vec![
                Command::Emit(Event::PrimaryDeviceEnable, "false".to_string()),
                Command::Emit(Event::SecondaryDeviceEnable, "true".to_string()),
            ]
        );
    }
}

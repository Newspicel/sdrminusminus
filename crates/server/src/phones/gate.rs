use sdrmm_wire::{
    API_PROTOCOL, MdnsState, PhoneAccess, PhoneAccessStatus, PhoneEndpoint, PhoneListenerState,
};

#[derive(Default)]
pub(crate) struct PhoneGate;

impl PhoneGate {
    pub(crate) fn status(&self) -> PhoneAccessStatus {
        PhoneAccessStatus {
            access: PhoneAccess::default(),
            listener: PhoneListenerState::Off,
            endpoint: self.endpoint(),
            mdns: MdnsState::Off,
            protocol: API_PROTOCOL,
        }
    }

    pub(crate) fn endpoint(&self) -> Option<PhoneEndpoint> {
        None
    }
}

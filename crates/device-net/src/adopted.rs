//! The endpoints a driver has been told about.
use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

use crate::endpoint::Endpoint;

/// Endpoints one driver will answer for. Bounded so a client that can post device ids cannot make
/// the device list grow without end; well above any plausible number of remote receivers.
const MAX_ENDPOINTS: usize = 64;

#[derive(Debug, Default)]
pub(crate) struct Adopted {
    endpoints: Mutex<BTreeSet<Endpoint>>,
}

impl Adopted {
    /// Take responsibility for `endpoint`, so later probes report it. `false` once the list is
    /// full and the endpoint is not already on it — the caller refuses rather than silently
    /// opening a radio no probe will ever confirm, which the engine would fault seconds later.
    pub(crate) fn adopt(&self, endpoint: Endpoint) -> bool {
        let mut endpoints = self.lock();
        endpoints.contains(&endpoint)
            || endpoints.len() < MAX_ENDPOINTS && endpoints.insert(endpoint)
    }

    /// Every adopted endpoint, in a stable order.
    pub(crate) fn list(&self) -> Vec<Endpoint> {
        self.lock().iter().cloned().collect()
    }

    /// A driver's endpoint list carries no state a panic could half-write, and losing every remote
    /// radio for the rest of the session over one is the worse outcome.
    fn lock(&self) -> MutexGuard<'_, BTreeSet<Endpoint>> {
        self.endpoints
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(host: &str) -> Endpoint {
        Endpoint::parse(host, 1234).expect("parses")
    }

    #[test]
    fn an_adopted_endpoint_is_listed_once_however_often_it_is_adopted() {
        let adopted = Adopted::default();
        assert!(adopted.adopt(endpoint("b.local")));
        assert!(adopted.adopt(endpoint("a.local")));
        assert!(adopted.adopt(endpoint("b.local")));
        assert_eq!(
            adopted
                .list()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec!["a.local:1234".to_string(), "b.local:1234".to_string()]
        );
    }

    #[test]
    fn the_list_is_bounded_but_never_refuses_one_it_already_holds() {
        let adopted = Adopted::default();
        for n in 0..MAX_ENDPOINTS {
            assert!(adopted.adopt(endpoint(&format!("host{n}.local"))));
        }
        assert!(!adopted.adopt(endpoint("one.too.many")));
        assert!(adopted.adopt(endpoint("host0.local")), "already held");
        assert_eq!(adopted.list().len(), MAX_ENDPOINTS);
    }
}

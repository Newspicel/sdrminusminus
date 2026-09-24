use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

use crate::net::Endpoint;

const MAX_ENDPOINTS: usize = 64;

/// The network addresses a driver has been told about, since nothing announces them.
///
/// A backend that addresses more than a host, one tuner of a dual-tuner receiver, say, adopts
/// its own key type rather than losing the difference between two devices at the same endpoint.
#[derive(Debug)]
pub struct Adopted<K = Endpoint> {
    keys: Mutex<BTreeSet<K>>,
}

impl<K: Ord> Default for Adopted<K> {
    fn default() -> Self {
        Self {
            keys: Mutex::new(BTreeSet::new()),
        }
    }
}

impl<K: Clone + Ord> Adopted<K> {
    pub fn adopt(&self, key: K) -> bool {
        let mut keys = self.lock();
        keys.contains(&key) || keys.len() < MAX_ENDPOINTS && keys.insert(key)
    }

    pub fn list(&self) -> Vec<K> {
        self.lock().iter().cloned().collect()
    }

    fn lock(&self) -> MutexGuard<'_, BTreeSet<K>> {
        self.keys.lock().unwrap_or_else(PoisonError::into_inner)
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

    #[test]
    fn a_driver_that_addresses_more_than_a_host_keeps_each_key_apart() {
        let adopted: Adopted<(Endpoint, u8)> = Adopted::default();
        assert!(adopted.adopt((endpoint("duo.local"), 0)));
        assert!(adopted.adopt((endpoint("duo.local"), 1)));
        assert_eq!(adopted.list().len(), 2);
    }
}

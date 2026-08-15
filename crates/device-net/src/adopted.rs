use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

use crate::endpoint::Endpoint;

const MAX_ENDPOINTS: usize = 64;

#[derive(Debug, Default)]
pub(crate) struct Adopted {
    endpoints: Mutex<BTreeSet<Endpoint>>,
}

impl Adopted {
    pub(crate) fn adopt(&self, endpoint: Endpoint) -> bool {
        let mut endpoints = self.lock();
        endpoints.contains(&endpoint)
            || endpoints.len() < MAX_ENDPOINTS && endpoints.insert(endpoint)
    }

    pub(crate) fn list(&self) -> Vec<Endpoint> {
        self.lock().iter().cloned().collect()
    }

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

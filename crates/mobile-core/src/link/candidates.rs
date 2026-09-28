use std::{
    collections::VecDeque,
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};

use futures::{StreamExt, stream::FuturesUnordered};

use super::DialError;

pub(crate) const STAGGER: Duration = Duration::from_millis(300);
pub(crate) const MAX_IN_FLIGHT: usize = 4;

pub(crate) fn ordered(hosts: &[String], last_good: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = last_good
        .filter(|good| hosts.iter().any(|host| host == good))
        .map(str::to_owned)
        .into_iter()
        .collect();
    for host in hosts {
        if !out.contains(host) {
            out.push(host.clone());
        }
    }
    out
}

pub(crate) fn merged(hosts: &[String], fresh: &[String], limit: usize) -> Vec<String> {
    let (first, older) = hosts.split_at(hosts.len().min(1));
    let mut out: Vec<String> = Vec::with_capacity(hosts.len() + fresh.len());
    for host in first.iter().chain(fresh).chain(older) {
        if !out.contains(host) {
            out.push(host.clone());
        }
    }
    out.truncate(limit);
    out
}

pub(crate) fn promoted(hosts: &[String], winner: &str) -> Vec<String> {
    ordered(hosts, Some(winner))
}

pub(crate) async fn race<F, Fut, T>(
    hosts: &[String],
    stagger: Duration,
    attempt: F,
) -> Result<(String, T), Vec<(String, DialError)>>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, DialError>>,
{
    let mut pending: VecDeque<String> = hosts.iter().cloned().collect();
    let mut running = FuturesUnordered::new();
    let mut failures = Vec::new();
    let launch = |host: String| {
        let future = attempt(host.clone());
        async move { (host, future.await) }
    };
    if let Some(host) = pending.pop_front() {
        running.push(launch(host));
    }
    loop {
        if running.is_empty() {
            match pending.pop_front() {
                Some(host) => running.push(launch(host)),
                None => return Err(failures),
            }
        }
        let next_start = tokio::time::sleep(stagger);
        tokio::select! {
            finished = running.next() => match finished {
                Some((host, Ok(value))) => return Ok((host, value)),
                Some((host, Err(error))) => {
                    failures.push((host, error));
                    if let Some(host) = pending.pop_front() {
                        running.push(launch(host));
                    }
                }
                None => {}
            },
            () = next_start, if running.len() < MAX_IN_FLIGHT && !pending.is_empty() => {
                if let Some(host) = pending.pop_front() {
                    running.push(launch(host));
                }
            }
        }
    }
}

pub(crate) fn worst(failures: Vec<(String, DialError)>) -> DialError {
    failures
        .into_iter()
        .map(|(_, error)| error)
        .max_by_key(DialError::rank)
        .unwrap_or_else(|| DialError::Unreachable("no host".to_owned()))
}

pub(crate) fn is_local(host: &str) -> bool {
    let name = host_name(host);
    if name.ends_with(".local") {
        return true;
    }
    match name.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => local_v4(address),
        Ok(IpAddr::V6(address)) => local_v6(address),
        Err(_) => false,
    }
}

pub(crate) fn host_name(host: &str) -> &str {
    if let Some(bracketed) = host.strip_prefix('[') {
        return bracketed
            .split_once(']')
            .map_or(bracketed, |(name, _)| name);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

fn local_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    address.is_private()
        || address.is_link_local()
        || address.is_loopback()
        || (first == 100 && (64..128).contains(&second))
}

fn local_v6(address: Ipv6Addr) -> bool {
    let first = address.segments()[0];
    address.is_loopback() || (first & 0xffc0) == 0xfe80 || (first & 0xfe00) == 0xfc00
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use tokio::time::Instant;

    use super::*;

    fn hosts(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn candidates_try_the_last_good_host_first() {
        let list = hosts(&["a:1", "b:1", "c:1"]);
        assert_eq!(ordered(&list, Some("c:1")), hosts(&["c:1", "a:1", "b:1"]));
        assert_eq!(ordered(&list, None), list);
        assert_eq!(ordered(&list, Some("gone:1")), list);
        assert_eq!(promoted(&list, "b:1"), hosts(&["b:1", "a:1", "c:1"]));
    }

    #[test]
    fn fresh_hosts_follow_the_last_good_one_and_push_out_the_oldest() {
        let list = hosts(&["a:1", "b:1", "c:1"]);
        assert_eq!(
            merged(&list, &hosts(&["b:1", "d:1"]), 8),
            hosts(&["a:1", "b:1", "d:1", "c:1"])
        );
        assert_eq!(
            merged(&list, &hosts(&["d:1", "e:1"]), 4),
            hosts(&["a:1", "d:1", "e:1", "b:1"])
        );
        assert_eq!(merged(&[], &hosts(&["d:1", "d:1"]), 4), hosts(&["d:1"]));
        assert_eq!(merged(&list, &[], 8), list);
    }

    #[tokio::test(start_paused = true)]
    async fn candidates_race_with_a_stagger_and_the_first_answer_wins() {
        let started = Arc::new(Mutex::new(Vec::new()));
        let origin = Instant::now();
        let list = hosts(&["slow:1", "fast:1", "late:1"]);
        let result = race(&list, STAGGER, |host| {
            let started = started.clone();
            async move {
                started
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((host.clone(), origin.elapsed()));
                let wait = if host == "fast:1" { 400 } else { 5_000 };
                tokio::time::sleep(Duration::from_millis(wait)).await;
                Ok::<_, DialError>(host)
            }
        })
        .await;
        assert_eq!(result, Ok(("fast:1".to_owned(), "fast:1".to_owned())));
        let started = started
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        assert_eq!(
            started,
            vec![
                ("slow:1".to_owned(), Duration::ZERO),
                ("fast:1".to_owned(), STAGGER),
                ("late:1".to_owned(), STAGGER * 2)
            ]
        );
        assert_eq!(origin.elapsed(), STAGGER + Duration::from_millis(400));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_starts_the_next_host_at_once() {
        let origin = Instant::now();
        let list = hosts(&["bad:1", "good:1"]);
        let result = race(&list, STAGGER, |host| async move {
            if host == "bad:1" {
                Err(DialError::Unreachable("refused".to_owned()))
            } else {
                Ok(host)
            }
        })
        .await;
        assert_eq!(result.map(|(host, _)| host), Ok("good:1".to_owned()));
        assert_eq!(origin.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn when_all_hosts_fail_the_most_serious_problem_is_reported() {
        let list = hosts(&["a:1", "b:1", "c:1", "d:1"]);
        let result: Result<(String, ()), _> = race(&list, STAGGER, |host| async move {
            Err(match host.as_str() {
                "a:1" => DialError::TimedOut,
                "b:1" => DialError::KeyMismatch {
                    seen: "ab".to_owned(),
                },
                "c:1" => DialError::Unreachable("no route".to_owned()),
                _ => DialError::Server {
                    status: 502,
                    message: "bad gateway".to_owned(),
                },
            })
        })
        .await;
        let failures = result.expect_err("all fail");
        assert_eq!(failures.len(), 4);
        assert_eq!(
            worst(failures),
            DialError::KeyMismatch {
                seen: "ab".to_owned()
            }
        );
        assert_eq!(
            worst(vec![
                (
                    "a".to_owned(),
                    DialError::KeyMismatch {
                        seen: String::new()
                    }
                ),
                ("b".to_owned(), DialError::Protocol { server: 2 }),
                ("c".to_owned(), DialError::Revoked),
            ]),
            DialError::Revoked
        );
    }

    #[test]
    fn local_addresses_are_recognised() {
        for local in [
            "192.168.1.20:8443",
            "10.0.0.2:1",
            "172.16.4.4:1",
            "169.254.1.1:1",
            "100.100.1.1:1",
            "pi.local:8443",
            "[fe80::1]:8443",
            "[fd00::2]:8443",
        ] {
            assert!(is_local(local), "{local}");
        }
        for remote in ["8.8.8.8:443", "example.org:443", "[2001:db8::1]:443"] {
            assert!(!is_local(remote), "{remote}");
        }
        assert_eq!(host_name("[fe80::1]:8443"), "fe80::1");
        assert_eq!(host_name("pi.local:8443"), "pi.local");
    }
}

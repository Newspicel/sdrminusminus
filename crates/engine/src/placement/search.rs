use std::time::{Duration, Instant};

use super::{coverage::Coverage, model::Domain};

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub duration: Duration,
    pub nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            duration: Duration::from_millis(50),
            nodes: 100_000,
        }
    }
}

pub(super) struct Budget {
    started: Instant,
    limits: Limits,
    pub nodes: usize,
}

impl Budget {
    pub(super) fn new(limits: Limits) -> Self {
        Self {
            started: Instant::now(),
            limits,
            nodes: 0,
        }
    }

    pub(super) fn expired(&self) -> bool {
        self.nodes >= self.limits.nodes || self.started.elapsed() >= self.limits.duration
    }
}

pub(super) struct Search<'a> {
    domains: &'a [Domain],
    suffix: Vec<Coverage>,
    pub best: usize,
    pub chosen: Option<Vec<usize>>,
    pub upper_bound: usize,
    pub budget: Budget,
    interrupted: bool,
}

impl<'a> Search<'a> {
    pub(super) fn new(domains: &'a [Domain], total: usize, best: usize, budget: Budget) -> Self {
        let mut suffix = vec![Coverage::empty(total); domains.len() + 1];
        for index in (0..domains.len()).rev() {
            suffix[index] = domains[index]
                .options
                .iter()
                .fold(suffix[index + 1].clone(), |all, option| all.union(option));
        }
        Self {
            domains,
            suffix,
            best,
            chosen: None,
            upper_bound: total,
            budget,
            interrupted: false,
        }
    }

    fn bound(&self, depth: usize, covered: &Coverage) -> usize {
        let union = covered.union(&self.suffix[depth]).count();
        let independent = covered.count()
            + self.domains[depth..]
                .iter()
                .map(|domain| {
                    domain
                        .options
                        .iter()
                        .map(|option| option.gain(covered))
                        .max()
                        .unwrap_or(0)
                })
                .sum::<usize>();
        union.min(independent)
    }

    pub(super) fn run(&mut self, total: usize) {
        let empty = Coverage::empty(total);
        self.upper_bound = self.bound(0, &empty);
        if self.best > self.upper_bound {
            self.upper_bound = total;
            return;
        }
        self.visit(0, &empty, &mut Vec::with_capacity(self.domains.len()));
        if !self.interrupted {
            self.upper_bound = self.best;
        }
    }

    fn visit(&mut self, depth: usize, covered: &Coverage, chosen: &mut Vec<usize>) {
        if self.bound(depth, covered) <= self.best {
            return;
        }
        if self.budget.expired() {
            self.interrupted = true;
            return;
        }
        self.budget.nodes += 1;
        if depth == self.domains.len() {
            self.best = covered.count();
            self.chosen = Some(chosen.clone());
            return;
        }
        let domain = &self.domains[depth];
        let mut order: Vec<_> = (0..domain.options.len()).collect();
        order.sort_by_key(|&index| {
            (
                std::cmp::Reverse(domain.options[index].gain(covered)),
                index,
            )
        });
        for index in order {
            if self.interrupted || self.best >= self.upper_bound {
                return;
            }
            let next = domain.options[index].union(covered);
            chosen.push(index);
            self.visit(depth + 1, &next, chosen);
            chosen.pop();
        }
    }
}

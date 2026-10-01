use super::GROUP;

const NORMALIZE: f32 = 0.875;

struct Csr {
    offsets: Vec<usize>,
    values: Vec<usize>,
}

impl Csr {
    fn build(count: usize, edges: &[(usize, usize)]) -> Self {
        let mut offsets = vec![0; count + 1];
        for &(key, _) in edges {
            offsets[key + 1] += 1;
        }
        for index in 0..count {
            offsets[index + 1] += offsets[index];
        }
        let mut cursor = offsets.clone();
        let mut values = vec![0; edges.len()];
        for &(key, value) in edges {
            values[cursor[key]] = value;
            cursor[key] += 1;
        }
        Self { offsets, values }
    }

    fn row(&self, index: usize) -> &[usize] {
        &self.values[self.offsets[index]..self.offsets[index + 1]]
    }

    fn rows(&self) -> usize {
        self.offsets.len() - 1
    }
}

pub(super) struct Flooding {
    information: usize,
    checks: Csr,
    variables: Csr,
    check_to_variable: Vec<f32>,
    variable_to_check: Vec<f32>,
    totals: Vec<f32>,
}

impl Flooding {
    pub(super) fn new(length: usize, addresses: &[&[u16]]) -> Self {
        let information = addresses.len() * GROUP;
        let parity = length - information;
        let step = parity / GROUP;
        let mut edges = Vec::new();
        for bit in 0..information {
            for &address in addresses[bit / GROUP] {
                let check = (usize::from(address) + (bit % GROUP) * step) % parity;
                edges.push((check, bit));
            }
        }
        for check in 0..parity {
            edges.push((check, information + check));
            if check > 0 {
                edges.push((check, information + check - 1));
            }
        }
        edges.sort_unstable();
        let checks = Csr::build(parity, &edges);
        let mirrored: Vec<(usize, usize)> = (0..parity)
            .flat_map(|check| {
                let start = checks.offsets[check];
                checks
                    .row(check)
                    .iter()
                    .enumerate()
                    .map(move |(offset, &variable)| (variable, start + offset))
            })
            .collect();
        let variables = Csr::build(length, &mirrored);
        let count = checks.values.len();
        Self {
            information,
            checks,
            variables,
            check_to_variable: vec![0.0; count],
            variable_to_check: vec![0.0; count],
            totals: vec![0.0; length],
        }
    }

    pub(super) fn edges(&self) -> Vec<(usize, usize)> {
        (0..self.checks.rows())
            .flat_map(|check| self.checks.row(check).iter().map(move |&bit| (check, bit)))
            .collect()
    }

    pub(super) fn satisfies(&self, codeword: &[bool]) -> bool {
        (0..self.checks.rows()).all(|check| {
            !self
                .checks
                .row(check)
                .iter()
                .fold(false, |parity, &variable| parity ^ codeword[variable])
        })
    }

    pub(super) fn decode(&mut self, llrs: &[f32], limit: usize) -> Option<(usize, Vec<bool>)> {
        self.check_to_variable.fill(0.0);
        for iteration in 0..=limit {
            self.update_variables(llrs);
            let hard: Vec<bool> = self.totals.iter().map(|&total| total < 0.0).collect();
            if self.satisfies(&hard) {
                return Some((iteration, hard[..self.information].to_vec()));
            }
            self.update_checks();
        }
        None
    }

    fn update_variables(&mut self, llrs: &[f32]) {
        for (variable, &llr) in llrs.iter().enumerate() {
            let edges = self.variables.row(variable);
            let total = llr
                + edges
                    .iter()
                    .map(|&edge| self.check_to_variable[edge])
                    .sum::<f32>();
            self.totals[variable] = total;
            for &edge in edges {
                self.variable_to_check[edge] = total - self.check_to_variable[edge];
            }
        }
    }

    fn update_checks(&mut self) {
        for check in 0..self.checks.rows() {
            let edges = self.checks.offsets[check]..self.checks.offsets[check + 1];
            let mut negative = false;
            let mut smallest = f32::INFINITY;
            let mut second = f32::INFINITY;
            for edge in edges.clone() {
                let value = self.variable_to_check[edge];
                negative ^= value < 0.0;
                let magnitude = value.abs();
                second = second.min(smallest.max(magnitude));
                smallest = smallest.min(magnitude);
            }
            for edge in edges {
                let value = self.variable_to_check[edge];
                let magnitude = if value.abs() == smallest {
                    second
                } else {
                    smallest
                };
                let outgoing = if negative ^ (value < 0.0) {
                    -magnitude
                } else {
                    magnitude
                };
                self.check_to_variable[edge] = NORMALIZE * outgoing;
            }
        }
    }
}

use super::RadarDspError;
use super::cfar::{Hit, Strongest};

pub const MAX_SNAPSHOTS: usize = 5;

const INTERPOLATION_FLOOR: f32 = 1e-30;
const STACK_RESERVE: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cluster {
    pub gate: f32,
    pub row: f32,
    pub centroid_gate: f32,
    pub centroid_row: f32,
    pub peak_gate: u32,
    pub peak_row: u32,
    pub power: f32,
    pub noise: f32,
    pub cells: u32,
    pub rows_spanned: u32,
    pub ridge: bool,
    pub snapshots: [(u32, u32); MAX_SNAPSHOTS],
    pub snapshot_count: u8,
}

#[derive(Clone, Copy, Debug)]
struct Growth {
    peak: Hit,
    weight: f64,
    row_sum: f64,
    gate_sum: f64,
    cells: u32,
    min_row: u32,
    max_row: u32,
    strongest: [(f32, u32, u32); MAX_SNAPSHOTS],
    held: usize,
}

impl Growth {
    fn new(seed: Hit) -> Self {
        Self {
            peak: seed,
            weight: 0.0,
            row_sum: 0.0,
            gate_sum: 0.0,
            cells: 0,
            min_row: seed.row,
            max_row: seed.row,
            strongest: [(0.0, 0, 0); MAX_SNAPSHOTS],
            held: 0,
        }
    }

    fn absorb(&mut self, hit: Hit) {
        if hit.power > self.peak.power {
            self.peak = hit;
        }
        let weight = f64::from(hit.power.max(0.0));
        self.weight += weight;
        self.row_sum += weight * f64::from(hit.row);
        self.gate_sum += weight * f64::from(hit.gate);
        self.cells += 1;
        self.min_row = self.min_row.min(hit.row);
        self.max_row = self.max_row.max(hit.row);
        let entry = (hit.power, hit.row, hit.gate);
        if self.held < MAX_SNAPSHOTS {
            self.strongest[self.held] = entry;
            self.held += 1;
        } else if let Some(weakest) = self.strongest.iter_mut().min_by(|a, b| a.0.total_cmp(&b.0))
            && weakest.0 < hit.power
        {
            *weakest = entry;
        }
    }

    fn centroid(&self) -> (f32, f32) {
        if self.weight > 0.0 {
            (
                (self.row_sum / self.weight) as f32,
                (self.gate_sum / self.weight) as f32,
            )
        } else {
            (self.peak.row as f32, self.peak.gate as f32)
        }
    }
}

pub struct Clusterer {
    gates: usize,
    rows: usize,
    marks: Vec<u32>,
    stack: Vec<usize>,
}

impl Clusterer {
    pub fn new(gates: usize, rows: usize) -> Result<Self, RadarDspError> {
        if gates == 0 || rows == 0 || u32::try_from(gates * rows).is_err() {
            return Err(RadarDspError::Setting);
        }
        Ok(Self {
            gates,
            rows,
            marks: vec![0; gates * rows],
            stack: Vec::with_capacity((gates * rows).min(STACK_RESERVE)),
        })
    }

    pub fn cluster(
        &mut self,
        hits: &[Hit],
        power: &[f32],
        ridge_rows: usize,
        out: &mut Vec<Cluster>,
        capacity: usize,
    ) -> Result<usize, RadarDspError> {
        out.clear();
        if power.len() < self.rows * self.gates {
            return Err(RadarDspError::Shape);
        }
        out.reserve(capacity);
        self.stack.reserve(hits.len());
        let mut keep = Strongest::new(capacity);
        let mut skipped = 0;
        for (index, hit) in hits.iter().enumerate() {
            match self.cell(hit) {
                Some(cell) => self.marks[cell] = index as u32 + 1,
                None => skipped += 1,
            }
        }
        for hit in hits {
            let Some(cell) = self.cell(hit) else {
                continue;
            };
            if self.marks[cell] == 0 {
                continue;
            }
            let growth = self.grow(cell, hits);
            let cluster = self.finish(&growth, power, ridge_rows);
            keep.offer(out, cluster, |cluster| cluster.power);
        }
        out.sort_unstable_by(|a, b| b.power.total_cmp(&a.power));
        Ok(keep.dropped() + skipped)
    }

    fn cell(&self, hit: &Hit) -> Option<usize> {
        let (row, gate) = (hit.row as usize, hit.gate as usize);
        (row < self.rows && gate < self.gates).then_some(row * self.gates + gate)
    }

    fn grow(&mut self, seed: usize, hits: &[Hit]) -> Growth {
        let first = hits[self.marks[seed] as usize - 1];
        let mut growth = Growth::new(first);
        self.stack.clear();
        self.take(seed, hits, &mut growth);
        while let Some(cell) = self.stack.pop() {
            let (row, gate) = (cell / self.gates, cell % self.gates);
            for next_row in row.saturating_sub(1)..=(row + 1).min(self.rows - 1) {
                for next_gate in gate.saturating_sub(1)..=(gate + 1).min(self.gates - 1) {
                    self.take(next_row * self.gates + next_gate, hits, &mut growth);
                }
            }
        }
        growth
    }

    fn take(&mut self, cell: usize, hits: &[Hit], growth: &mut Growth) {
        let mark = std::mem::take(&mut self.marks[cell]);
        if let Some(hit) = (mark as usize)
            .checked_sub(1)
            .and_then(|index| hits.get(index))
        {
            growth.absorb(*hit);
            self.stack.push(cell);
        }
    }

    fn finish(&self, growth: &Growth, power: &[f32], ridge_rows: usize) -> Cluster {
        let peak = growth.peak;
        let (row, gate) = (peak.row as usize, peak.gate as usize);
        let level = |r: usize, g: usize| {
            power
                .get(r * self.gates + g)
                .map(|value| 10.0 * value.max(INTERPOLATION_FLOOR).log10())
        };
        let across_gates = parabolic(
            gate.checked_sub(1).and_then(|g| level(row, g)),
            level(row, gate),
            (gate + 1 < self.gates)
                .then(|| level(row, gate + 1))
                .flatten(),
        );
        let across_rows = parabolic(
            row.checked_sub(1).and_then(|r| level(r, gate)),
            level(row, gate),
            (row + 1 < self.rows)
                .then(|| level(row + 1, gate))
                .flatten(),
        );
        let (centroid_row, centroid_gate) = growth.centroid();
        let mut snapshots = [(0u32, 0u32); MAX_SNAPSHOTS];
        let mut ranked = growth.strongest;
        ranked[..growth.held].sort_unstable_by(|a, b| b.0.total_cmp(&a.0));
        for (slot, entry) in snapshots.iter_mut().zip(&ranked[..growth.held]) {
            *slot = (entry.1, entry.2);
        }
        let rows_spanned = growth.max_row - growth.min_row + 1;
        Cluster {
            gate: gate as f32 + across_gates,
            row: row as f32 + across_rows,
            centroid_gate,
            centroid_row,
            peak_gate: peak.gate,
            peak_row: peak.row,
            power: peak.power,
            noise: peak.noise,
            cells: growth.cells,
            rows_spanned,
            ridge: rows_spanned as usize > ridge_rows,
            snapshots,
            snapshot_count: growth.held as u8,
        }
    }
}

fn parabolic(minus: Option<f32>, centre: Option<f32>, plus: Option<f32>) -> f32 {
    let (Some(minus), Some(centre), Some(plus)) = (minus, centre, plus) else {
        return 0.0;
    };
    let curvature = minus - 2.0 * centre + plus;
    if curvature >= 0.0 || curvature.is_nan() {
        return 0.0;
    }
    ((minus - plus) / (2.0 * curvature)).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: usize = 32;
    const GATES: usize = 64;

    fn hit(row: u32, gate: u32, power: f32) -> Hit {
        Hit {
            row,
            gate,
            power,
            noise: 1.0,
        }
    }

    fn map_of(hits: &[Hit]) -> Vec<f32> {
        let mut power = vec![1.0f32; ROWS * GATES];
        for hit in hits {
            power[hit.row as usize * GATES + hit.gate as usize] = hit.power;
        }
        power
    }

    fn run(
        hits: &[Hit],
        power: &[f32],
        ridge_rows: usize,
        capacity: usize,
    ) -> (Vec<Cluster>, usize) {
        let mut clusterer = Clusterer::new(GATES, ROWS).unwrap();
        let mut out = Vec::new();
        let dropped = clusterer
            .cluster(hits, power, ridge_rows, &mut out, capacity)
            .unwrap();
        (out, dropped)
    }

    fn db(value: f32) -> f32 {
        10f32.powf(value / 10.0)
    }

    #[test]
    fn an_elongated_patch_is_one_cluster() {
        let hits: Vec<Hit> = [10.0, 11.0, 12.0, 11.0, 10.0]
            .iter()
            .enumerate()
            .map(|(k, &snr)| hit(8, 20 + k as u32, db(snr)))
            .collect();
        let (clusters, dropped) = run(&hits, &map_of(&hits), 8, 16);
        assert_eq!(dropped, 0);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].cells, 5);
        assert_eq!(clusters[0].peak_gate, 22);
        assert!((clusters[0].gate - 22.0).abs() < 1e-5);
        assert_eq!(clusters[0].snapshot_count, 5);
        assert_eq!(clusters[0].snapshots[0], (8, 22));
    }

    #[test]
    fn diagonal_cells_join() {
        let hits = [
            hit(4, 4, 50.0),
            hit(5, 5, 60.0),
            hit(6, 6, 40.0),
            hit(9, 9, 30.0),
        ];
        let (clusters, _) = run(&hits, &map_of(&hits), 8, 16);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0].cells, 3);
        assert_eq!((clusters[0].peak_row, clusters[0].peak_gate), (5, 5));
        assert_eq!(clusters[1].cells, 1);
    }

    #[test]
    fn centroid_is_power_weighted() {
        let hits = [hit(10, 30, 30.0), hit(10, 31, 10.0), hit(11, 30, 10.0)];
        let (clusters, _) = run(&hits, &map_of(&hits), 8, 16);
        let cluster = clusters[0];
        assert!((cluster.centroid_gate - 30.2).abs() < 1e-5);
        assert!((cluster.centroid_row - 10.2).abs() < 1e-5);
    }

    #[test]
    fn parabolic_peak_recovers_a_fractional_delay() {
        let mut power = vec![1.0f32; ROWS * GATES];
        let mut hits = Vec::new();
        for gate in 36..=44 {
            for row in 14..=18 {
                let offset = (gate as f32 - 40.3).powi(2) + (row as f32 - 16.0).powi(2);
                let value = 1e3 * (-offset / 4.0).exp();
                power[row * GATES + gate] = value;
                if value > 20.0 {
                    hits.push(hit(row as u32, gate as u32, value));
                }
            }
        }
        let (clusters, _) = run(&hits, &power, 8, 16);
        assert_eq!(clusters.len(), 1);
        assert!(
            (clusters[0].gate - 40.3).abs() < 0.1,
            "{}",
            clusters[0].gate
        );
        assert!((clusters[0].row - 16.0).abs() < 0.1);
    }

    #[test]
    fn a_row_spanning_patch_is_a_ridge() {
        let ridge: Vec<Hit> = (5..20).map(|row| hit(row, 3, 40.0)).collect();
        let (clusters, _) = run(&ridge, &map_of(&ridge), 8, 16);
        assert_eq!(clusters.len(), 1);
        assert!(clusters[0].ridge);
        assert_eq!(clusters[0].rows_spanned, 15);
        let compact = [hit(5, 10, 40.0), hit(6, 10, 30.0)];
        let (clusters, _) = run(&compact, &map_of(&compact), 8, 16);
        assert!(!clusters[0].ridge);
    }

    #[test]
    fn surplus_clusters_drop_the_weakest_and_count() {
        let hits: Vec<Hit> = (0..20)
            .map(|k| hit(2 + k / 5 * 3, 2 + k % 5 * 3, 10.0 + k as f32))
            .collect();
        let (clusters, dropped) = run(&hits, &map_of(&hits), 8, 8);
        assert_eq!(dropped, 12);
        assert_eq!(clusters.len(), 8);
        assert!(clusters.iter().all(|cluster| cluster.power >= 22.0));
        assert!(
            clusters
                .windows(2)
                .all(|pair| pair[0].power >= pair[1].power)
        );
        let outside = [hit(ROWS as u32, 0, 5.0)];
        let (clusters, dropped) = run(&outside, &map_of(&[]), 8, 8);
        assert!(clusters.is_empty());
        assert_eq!(dropped, 1);
        let mut clusterer = Clusterer::new(GATES, ROWS).unwrap();
        let mut out = Vec::new();
        assert_eq!(
            clusterer.cluster(&outside, &[1.0; 3], 8, &mut out, 8),
            Err(RadarDspError::Shape)
        );
    }

    #[test]
    fn edges_skip_interpolation() {
        let hits = [hit(0, 0, 100.0), hit(0, 1, 50.0)];
        let (clusters, _) = run(&hits, &map_of(&hits), 8, 8);
        assert_eq!(clusters[0].gate, 0.0);
        assert_eq!(clusters[0].row, 0.0);
    }
}

use std::hint::black_box;

use criterion::{Criterion, Throughput};
use num_complex::Complex;
use sdrmm_dsp::radar::{
    batch::{BatchKernel, BatchShape, DopplerTaper, WeightsAt},
    cfar::{Cfar, CfarSpec, Hit},
    cluster::{Cluster, Clusterer},
    threshold::CfarStatistic,
    wiener::{GroupPlan, GroupSums, WeightTable, WienerSolver},
};

use super::pseudo;

type C32 = Complex<f32>;

const FM_BATCHES: usize = 512;
const FM_BATCH_LEN: usize = 260;
const FM_GATES: usize = 73;
const ECA_LEAD: usize = 2;
const ECA_TAPS: usize = 14;
const GROUP_BATCHES: usize = 32;

struct Plane {
    kernel: BatchKernel,
    shape: BatchShape,
    reference: Vec<C32>,
    surveillance: Vec<C32>,
    spectrum: Vec<C32>,
    model: Vec<C32>,
    product: Vec<C32>,
    gates: Vec<C32>,
    cells: Vec<C32>,
    series: Vec<C32>,
    taper: Vec<f32>,
}

impl Plane {
    fn new(shape: BatchShape) -> Self {
        let m = shape.fft_len;
        let mut taper = vec![0.0f32; shape.batches];
        DopplerTaper::Hann.fill(&mut taper);
        Self {
            kernel: BatchKernel::new(shape),
            shape,
            reference: pseudo(shape.window(), 0x5EF),
            surveillance: pseudo(shape.window(), 0x5E1),
            spectrum: vec![C32::default(); m],
            model: vec![C32::default(); m],
            product: vec![C32::default(); m],
            gates: vec![C32::default(); shape.gates],
            cells: vec![C32::default(); shape.gates * shape.batches],
            series: vec![C32::default(); shape.batches],
            taper,
        }
    }

    fn cpi(&mut self, table: Option<&WeightTable>) -> f32 {
        let (gates, batches) = (self.shape.gates, self.shape.batches);
        for batch in 0..batches {
            self.kernel
                .reference(&self.reference, batch, &mut self.spectrum, &mut self.model)
                .expect("a reference batch");
            self.kernel
                .surveillance(&self.surveillance, batch, &self.spectrum, &mut self.product)
                .expect("a surveillance batch");
            let weights = table.map_or_else(WeightsAt::none, |table| table.at(0, batch));
            self.kernel
                .residual(&self.product, &self.model, weights, &mut self.gates)
                .expect("residual gates");
            for (gate, value) in self.gates.iter().enumerate() {
                self.cells[gate * batches + batch] = *value;
            }
        }
        for gate in 0..gates {
            self.series
                .copy_from_slice(&self.cells[gate * batches..(gate + 1) * batches]);
            self.kernel
                .doppler(&mut self.series, &self.taper)
                .expect("a doppler row");
        }
        self.series[0].norm()
    }
}

fn caf(c: &mut Criterion) {
    let shape = BatchShape::new(FM_BATCHES, FM_BATCH_LEN, FM_GATES, 0, 0, 1).expect("a shape");
    let mut plane = Plane::new(shape);
    let mut group = c.benchmark_group("radar");
    group.sample_size(20);
    group.throughput(Throughput::Elements((FM_BATCHES * FM_BATCH_LEN) as u64));
    group.bench_function("caf_fm_cpi", |b| b.iter(|| black_box(plane.cpi(None))));
    group.finish();
}

fn eca(c: &mut Criterion) {
    let shape = BatchShape::new(FM_BATCHES, FM_BATCH_LEN, FM_GATES, ECA_LEAD, ECA_TAPS, 1)
        .expect("a shape");
    let plan =
        GroupPlan::split(FM_BATCHES, GROUP_BATCHES, 0, 1, true, false).expect("a group plan");
    let mut solver = WienerSolver::new(shape, &plan, 1e-4).expect("a solver");
    let mut sums = GroupSums::new(&shape, &plan);
    let mut table = WeightTable::new(&shape, &plan).expect("a table");
    let mut plane = Plane::new(shape);
    let energy = [1.0f64];
    let mut group = c.benchmark_group("radar");
    group.sample_size(20);
    group.throughput(Throughput::Elements((FM_BATCHES * FM_BATCH_LEN) as u64));
    group.bench_function("eca_fm_solve", |b| {
        b.iter(|| {
            sums.clear();
            for batch in 0..FM_BATCHES {
                plane
                    .kernel
                    .reference(
                        &plane.reference,
                        batch,
                        &mut plane.spectrum,
                        &mut plane.model,
                    )
                    .expect("a reference batch");
                plane
                    .kernel
                    .surveillance(
                        &plane.surveillance,
                        batch,
                        &plane.spectrum,
                        &mut plane.product,
                    )
                    .expect("a surveillance batch");
                solver
                    .accumulate(&mut sums, batch, &plane.model, &plane.product, &energy)
                    .expect("an accumulation");
            }
            black_box(solver.solve(&sums, &mut table).expect("a solve"))
        });
    });
    group.bench_function("eca_fm_cpi", |b| {
        b.iter(|| black_box(plane.cpi(Some(&table))))
    });
    group.finish();
}

fn power_plane(rows: usize, gates: usize) -> Vec<f32> {
    let mut power: Vec<f32> = pseudo(rows * gates, 0xCFA)
        .iter()
        .map(|cell| cell.norm_sqr() + 0.1)
        .collect();
    for target in 0..24 {
        power[((target * 37) % rows) * gates + (target * 11) % gates] = 400.0;
    }
    power
}

fn spec(stat: CfarStatistic, plane: bool) -> CfarSpec {
    CfarSpec {
        stat,
        plane,
        guard_range: 2,
        train_range: 8,
        guard_doppler: 1,
        train_doppler: 4,
        alpha: 8.0,
        alpha_edge: 9.0,
        min_snr: 1.0,
        min_gate: 1,
        clutter_half_rows: 1,
    }
}

fn cfar(c: &mut Criterion) {
    let power = power_plane(FM_BATCHES, FM_GATES);
    let mut hits: Vec<Hit> = Vec::with_capacity(256);
    let mut group = c.benchmark_group("radar");
    group.throughput(Throughput::Elements((FM_BATCHES * FM_GATES) as u64));
    for (name, stat, plane) in [
        ("cfar_ca_range", CfarStatistic::Ca, false),
        ("cfar_os_plane", CfarStatistic::Os { rank: 0.75 }, true),
    ] {
        let mut cfar = Cfar::new(spec(stat, plane), FM_GATES, FM_BATCHES).expect("a cfar");
        group.bench_function(name, |b| {
            b.iter(|| {
                black_box(
                    cfar.detect(black_box(&power), 0..FM_BATCHES, &mut hits, 256)
                        .expect("detections"),
                )
            });
        });
    }
    group.finish();
}

fn cluster(c: &mut Criterion) {
    let (rows, gates) = (FM_BATCHES, FM_GATES);
    let mut power = vec![1.0f32; rows * gates];
    let hits: Vec<Hit> = (0..256u32)
        .map(|index| {
            let (row, gate) = (4 + (index % 64) * 7, 2 + (index / 64) * 16 + index % 3);
            let power_at = 20.0 + (index % 17) as f32;
            power[row as usize * gates + gate as usize] = power_at;
            Hit {
                row,
                gate,
                power: power_at,
                noise: 1.0,
            }
        })
        .collect();
    let mut clusterer = Clusterer::new(gates, rows).expect("a clusterer");
    let mut clusters: Vec<Cluster> = Vec::with_capacity(256);
    let mut group = c.benchmark_group("radar");
    group.throughput(Throughput::Elements(hits.len() as u64));
    group.bench_function("cluster_256_hits", |b| {
        b.iter(|| {
            black_box(
                clusterer
                    .cluster(black_box(&hits), &power, 8, &mut clusters, 256)
                    .expect("clusters"),
            )
        });
    });
    group.finish();
}

pub(crate) fn benches(c: &mut Criterion) {
    caf(c);
    eca(c);
    cfar(c);
    cluster(c);
}

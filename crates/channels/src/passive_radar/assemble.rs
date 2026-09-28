use num_complex::Complex;
use sdrmm_dsp::radar::batch::MAX_SURVEILLANCE;
use sdrmm_wire::radar::ReferenceHealth;

type C32 = Complex<f32>;

const NANOS_PER_SECOND: f64 = 1e9;

pub struct CpiJob {
    pub generation: u64,
    pub seq: u64,
    pub start_index: u64,
    pub start_unix_ns: u64,
    pub phase_ready: bool,
    pub reference: ReferenceHealth,
    pub front_suppression_db: [f32; MAX_SURVEILLANCE],
    pub canceller_resets: u64,
    pub lanes: usize,
    pub window: usize,
    pub samples: Vec<C32>,
}

impl CpiJob {
    #[must_use]
    pub fn new(lanes: usize, window: usize) -> Self {
        Self {
            generation: 0,
            seq: 0,
            start_index: 0,
            start_unix_ns: 0,
            phase_ready: false,
            reference: ReferenceHealth::default(),
            front_suppression_db: [0.0; MAX_SURVEILLANCE],
            canceller_resets: 0,
            lanes,
            window,
            samples: vec![C32::default(); lanes * window],
        }
    }

    #[must_use]
    pub fn lane(&self, index: usize) -> &[C32] {
        let start = index * self.window;
        self.samples.get(start..start + self.window).unwrap_or(&[])
    }

    pub fn lane_mut(&mut self, index: usize) -> &mut [C32] {
        let start = index * self.window;
        self.samples
            .get_mut(start..start + self.window)
            .unwrap_or(&mut [])
    }
}

struct LaneRing {
    data: Vec<C32>,
    head: usize,
    capacity: usize,
    latency: usize,
    skip: usize,
}

impl LaneRing {
    fn new(capacity: usize, latency: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            head: 0,
            capacity,
            latency,
            skip: latency,
        }
    }

    fn available(&self) -> usize {
        self.data.len() - self.head
    }

    fn samples(&self, len: usize) -> &[C32] {
        &self.data[self.head..self.head + len]
    }

    fn push(&mut self, samples: &[C32]) -> usize {
        let dropped = self.skip.min(samples.len());
        self.skip -= dropped;
        let samples = &samples[dropped..];
        if self.data.len() + samples.len() > self.capacity && self.head > 0 {
            self.data.copy_within(self.head.., 0);
            self.data.truncate(self.data.len() - self.head);
            self.head = 0;
        }
        let room = self.capacity - self.data.len();
        let take = room.min(samples.len());
        self.data.extend_from_slice(&samples[..take]);
        samples.len() - take
    }

    fn advance(&mut self, count: usize) {
        self.head += count.min(self.available());
    }

    fn reset(&mut self) {
        self.data.clear();
        self.head = 0;
        self.skip = self.latency;
    }
}

pub struct CpiAssembler {
    rings: Vec<LaneRing>,
    window: usize,
    hop: usize,
    pre: usize,
    rate: f64,
    origin: u64,
    advanced: u64,
    anchor: (u64, u64),
    unready_until: u64,
    lost: u64,
}

impl CpiAssembler {
    #[must_use]
    pub fn new(
        window: usize,
        hop: usize,
        pre: usize,
        rate: f64,
        latency: &[usize],
        chunk: usize,
    ) -> Self {
        let longest = latency.iter().copied().max().unwrap_or(0);
        let capacity = window + longest + 2 * chunk;
        Self {
            rings: latency
                .iter()
                .map(|&delay| LaneRing::new(capacity, delay))
                .collect(),
            window,
            hop: hop.max(1),
            pre,
            rate,
            origin: 0,
            advanced: 0,
            anchor: (0, 0),
            unready_until: 0,
            lost: 0,
        }
    }

    #[must_use]
    pub const fn lanes(&self) -> usize {
        self.rings.len()
    }

    #[must_use]
    pub const fn window(&self) -> usize {
        self.window
    }

    #[must_use]
    pub const fn hop(&self) -> usize {
        self.hop
    }

    pub fn push(&mut self, lane: usize, samples: &[C32]) {
        if let Some(ring) = self.rings.get_mut(lane) {
            self.lost += ring.push(samples) as u64;
        } else {
            self.lost += samples.len() as u64;
        }
    }

    pub const fn anchor(&mut self, radar_index: u64, unix_ns: u64) {
        self.anchor = (radar_index, unix_ns);
    }

    pub fn mark_unready(&mut self, until: u64) {
        self.unready_until = self.unready_until.max(until);
    }

    pub fn restart(&mut self, origin: u64) {
        for ring in &mut self.rings {
            ring.reset();
        }
        self.origin = origin;
        self.advanced = 0;
        self.unready_until = 0;
    }

    #[must_use]
    pub fn buffered(&self) -> usize {
        self.rings
            .iter()
            .map(LaneRing::available)
            .min()
            .unwrap_or(0)
    }

    #[must_use]
    pub const fn lost(&self) -> u64 {
        self.lost
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        !self.rings.is_empty() && self.buffered() >= self.window
    }

    pub fn set_hop(&mut self, hop: usize) {
        self.hop = hop.max(1);
    }

    #[must_use]
    pub fn window_start(&self) -> u64 {
        self.origin + self.advanced
    }

    pub fn fill(&mut self, job: &mut CpiJob) -> bool {
        let lanes = self.rings.len();
        if !self.ready()
            || job.lanes != lanes
            || job.window != self.window
            || job.samples.len() != lanes * self.window
        {
            return false;
        }
        for (lane, ring) in self.rings.iter().enumerate() {
            job.lane_mut(lane)
                .copy_from_slice(ring.samples(self.window));
        }
        let start = self.window_start();
        job.start_index = start + self.pre as u64;
        job.start_unix_ns = self.unix_at(job.start_index);
        job.phase_ready = self.unready_until <= start;
        self.skip();
        true
    }

    pub fn skip(&mut self) {
        for ring in &mut self.rings {
            ring.advance(self.hop);
        }
        self.advanced += self.hop as u64;
    }

    fn unix_at(&self, radar_index: u64) -> u64 {
        let (anchor_index, anchor_ns) = self.anchor;
        let offset = (radar_index as f64 - anchor_index as f64) * NANOS_PER_SECOND / self.rate;
        anchor_ns.saturating_add_signed(offset.round() as i64)
    }
}

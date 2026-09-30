struct Shape {
    fft_len: u32,
    fft_bits: u32,
    batches: u32,
    batch_bits: u32,
    batch_len: u32,
    gates: u32,
    lead: u32,
    span: u32,
    order: u32,
    pre: u32,
    window: u32,
    lanes: u32,
    first_lane: u32,
    slots: u32,
    groups: u32,
    shifts: u32,
    taps: u32,
    vectors: u32,
    eca: u32,
    scale: f32,
}

struct Term {
    group: u32,
    tap: u32,
    factor: vec2<f32>,
}

@group(0) @binding(0) var<uniform> shape: Shape;
@group(0) @binding(1) var<storage, read> window: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> spectra: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read_write> sums: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read> turns: array<vec2<f32>>;
@group(0) @binding(5) var<storage, read> ranges: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> weights: array<vec2<f32>>;
@group(0) @binding(7) var<storage, read> terms: array<Term>;
@group(0) @binding(8) var<storage, read_write> cube: array<vec2<f32>>;
@group(0) @binding(9) var<storage, read> taper: array<f32>;

fn multiply(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn conjugate(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x, -a.y);
}

fn reversed(index: u32, bits: u32) -> u32 {
    return reverseBits(index) >> (32u - bits);
}

fn flat_index(id: vec3<u32>, groups: vec3<u32>) -> u32 {
    return id.x + id.y * groups.x * 256u;
}

@compute @workgroup_size(256)
fn scatter(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    let per_slot = shape.batches * shape.fft_len;
    if (index >= shape.slots * per_slot) { return; }
    let slot = index / per_slot;
    let batch = (index / shape.fft_len) % shape.batches;
    let m = index % shape.fft_len;
    var lane = 0u;
    var start = shape.pre + batch * shape.batch_len;
    var len = shape.batch_len;
    if (slot >= shape.first_lane) {
        lane = slot - shape.first_lane + 1u;
        start = shape.pre + batch * shape.batch_len - shape.lead;
        len = shape.batch_len + shape.span - 1u;
    } else if (slot == 1u) {
        start = batch * shape.batch_len;
        len = shape.batch_len + shape.span + shape.order - 2u;
    }
    var value = vec2<f32>(0.0, 0.0);
    if (m < len) {
        value = window[lane * shape.window + start + m];
    }
    spectra[(slot * shape.batches + batch) * shape.fft_len + reversed(m, shape.fft_bits)] = value;
}

@compute @workgroup_size(256)
fn products(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    let per_slot = shape.batches * shape.fft_len;
    if (index >= per_slot) { return; }
    let reference = conjugate(spectra[index]);
    for (var slot = 1u; slot < shape.slots; slot++) {
        let at = slot * per_slot + index;
        spectra[at] = multiply(spectra[at], reference);
    }
}

@compute @workgroup_size(256)
fn group_sums(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    if (index >= shape.groups * shape.vectors * shape.fft_len) { return; }
    let m = index % shape.fft_len;
    let vector = (index / shape.fft_len) % shape.vectors;
    let group = index / (shape.fft_len * shape.vectors);
    var slot = 1u;
    var turn = vector;
    if (vector >= shape.shifts) {
        let lane_tap = vector - shape.shifts;
        slot = shape.first_lane + lane_tap / shape.taps;
        turn = shape.shifts + lane_tap % shape.taps;
    }
    let range = ranges[group];
    let per_batch = shape.shifts + shape.taps;
    var total = vec2<f32>(0.0, 0.0);
    for (var batch = range.x; batch < range.y; batch++) {
        let phase = turns[(group * shape.batches + batch) * per_batch + turn];
        total += multiply(phase, spectra[(slot * shape.batches + batch) * shape.fft_len + m]);
    }
    sums[index] = total;
}

fn residual_at(lane: u32, batch: u32, base: u32, m: u32) -> vec2<f32> {
    let product = spectra[base + m];
    if (shape.eca == 0u) {
        return product * shape.scale;
    }
    var kernel = vec2<f32>(0.0, 0.0);
    for (var j = 0u; j < 2u * shape.taps; j++) {
        let term = terms[batch * 2u * shape.taps + j];
        let at = ((term.group * shape.lanes + lane) * shape.taps + term.tap) * shape.fft_len + m;
        kernel += multiply(term.factor, weights[at]);
    }
    let model = spectra[(shape.batches + batch) * shape.fft_len + m];
    return (product - multiply(kernel, model)) * shape.scale;
}

@compute @workgroup_size(256)
fn residual(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    let per_lane = shape.batches * shape.fft_len;
    if (index >= shape.lanes * per_lane) { return; }
    let m = index % shape.fft_len;
    let partner = reversed(m, shape.fft_bits);
    if (partner < m) { return; }
    let batch = (index / shape.fft_len) % shape.batches;
    let lane = index / per_lane;
    let base = ((shape.first_lane + lane) * shape.batches + batch) * shape.fft_len;
    let low = residual_at(lane, batch, base, m);
    let high = residual_at(lane, batch, base, partner);
    spectra[base + m] = high;
    spectra[base + partner] = low;
}

@compute @workgroup_size(256)
fn gather(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    if (index >= shape.lanes * shape.gates * shape.batches) { return; }
    let gate = index % shape.gates;
    let batch = (index / shape.gates) % shape.batches;
    let lane = index / (shape.gates * shape.batches);
    let source = ((shape.first_lane + lane) * shape.batches + batch) * shape.fft_len + gate + shape.lead;
    let row = (lane * shape.gates + gate) * shape.batches + reversed(batch, shape.batch_bits);
    cube[row] = spectra[source] * taper[batch];
}

@compute @workgroup_size(256)
fn shift(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    let index = flat_index(id, groups);
    let half = shape.batches / 2u;
    if (index >= shape.lanes * shape.gates * half) { return; }
    let low = index / half * shape.batches + index % half;
    let value = cube[low];
    cube[low] = cube[low + half];
    cube[low + half] = value;
}

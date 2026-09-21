@group(0) @binding(0) var<storage, read_write> data: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read> twiddles: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> params: array<u32>;
var<workgroup> tile: array<vec2<f32>, 1024>;

fn multiply(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}
fn twiddle(index: u32) -> vec2<f32> {
    let w = twiddles[index];
    return vec2<f32>(w.x, select(w.y, -w.y, params[2] != 0u));
}
@compute @workgroup_size(256)
fn local_fft(@builtin(local_invocation_id) local: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>) {
    let base = group.y * params[0] + group.x * 1024u;
    for (var i = local.x; i < 1024u; i += 256u) { tile[i] = data[base + i]; }
    workgroupBarrier();
    for (var span = 2u; span <= 1024u; span *= 2u) {
        let half = span / 2u;
        for (var b = local.x; b < 512u; b += 256u) {
            let offset = b % half;
            let even = b / half * span + offset;
            let a = tile[even];
            let v = multiply(tile[even + half], twiddle(offset * (params[0] / span)));
            tile[even] = a + v;
            tile[even + half] = a - v;
        }
        workgroupBarrier();
    }
    for (var i = local.x; i < 1024u; i += 256u) { data[base + i] = tile[i]; }
}
@compute @workgroup_size(256)
fn global_fft(@builtin(global_invocation_id) id: vec3<u32>) {
    let half = params[1] / 2u;
    let offset = id.x % half;
    let even = id.y * params[0] + id.x / half * params[1] + offset;
    let a = data[even];
    let b = multiply(data[even + half], twiddle(offset * (params[0] / params[1])));
    data[even] = a + b;
    data[even + half] = a - b;
}

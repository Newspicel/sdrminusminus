@group(0) @binding(0) var<storage, read> input: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> output: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> params: array<u32>;
@group(0) @binding(3) var<storage, read> window: array<f32>;

@compute @workgroup_size(256)
fn prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    let reversed = reverseBits(id.x) >> (32u - params[1]);
    output[id.y * params[0] + id.x] = input[id.y * params[0] + reversed] * window[reversed];
}
@compute @workgroup_size(256)
fn power(@builtin(global_invocation_id) id: vec3<u32>) {
    let raw = id.x * 2u;
    let base = id.y * params[0];
    let scale = bitcast<f32>(params[2]);
    let a = 20.0 * log2(length(input[base + raw]) * scale + 1e-12) * 0.30102999566;
    let b = 20.0 * log2(length(input[base + raw + 1u]) * scale + 1e-12) * 0.30102999566;
    let shifted = (raw + params[0] / 2u) % params[0];
    output[(base + shifted) / 2u] = vec2<f32>(a, b);
}

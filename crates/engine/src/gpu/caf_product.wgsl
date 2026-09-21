@group(0) @binding(0) var<storage, read> reference: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read> observed: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> output: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read> params: array<u32>;

@compute @workgroup_size(256)
fn product(@builtin(global_invocation_id) id: vec3<u32>) {
    let bin = reverseBits(id.x) >> (32u - params[4]);
    let a = observed[id.y * params[1] + bin];
    let b = reference[bin];
    output[id.y * params[1] + id.x] = vec2<f32>(a.x * b.x + a.y * b.y, a.y * b.x - a.x * b.y);
}

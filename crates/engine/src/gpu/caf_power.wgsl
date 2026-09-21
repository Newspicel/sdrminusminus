@group(0) @binding(0) var<storage, read> input: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<storage, read> params: array<u32>;

@compute @workgroup_size(256)
fn power(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params[2]) { return; }
    let value = input[id.y * params[1] + id.x] / f32(params[1]);
    output[(id.y + params[5]) * params[2] + id.x] = dot(value, value);
}

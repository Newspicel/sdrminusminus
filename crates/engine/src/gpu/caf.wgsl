@group(0) @binding(0) var<storage, read> input: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> output: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> params: array<u32>;

@compute @workgroup_size(256)
fn prepare(@builtin(global_invocation_id) id: vec3<u32>) {
    let reversed = reverseBits(id.x) >> (32u - params[4]);
    var value = vec2<f32>(0.0);
    if (reversed < params[0]) { value = input[id.y * params[0] + reversed]; }
    output[id.y * params[1] + id.x] = value;
}
@compute @workgroup_size(256)
fn correlate(@builtin(global_invocation_id) id: vec3<u32>) {
    let bin = reverseBits(id.x) >> (32u - params[4]);
    let row = f32(id.y + params[5]) - f32(params[3] - 1u) / 2.0;
    let shift = i32(round(row * f32(params[1]) / f32(params[0])));
    let shifted = u32(i32(bin) + shift) & (params[1] - 1u);
    let a = input[params[1] + shifted];
    let b = input[bin];
    output[id.y * params[1] + id.x] = vec2<f32>(a.x * b.x + a.y * b.y, a.y * b.x - a.x * b.y);
}
@compute @workgroup_size(256)
fn mix(@builtin(global_invocation_id) id: vec3<u32>) {
    let reversed = reverseBits(id.x) >> (32u - params[4]);
    var value = vec2<f32>(0.0);
    if (reversed < params[0]) {
        let row = f32(id.y + params[5]) - f32(params[3] - 1u) / 2.0;
        let phase = -6.28318530718 * row * f32(reversed) / f32(params[0]);
        let a = input[params[0] + reversed];
        let b = vec2<f32>(cos(phase), sin(phase));
        value = vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
    }
    output[id.y * params[1] + id.x] = value;
}

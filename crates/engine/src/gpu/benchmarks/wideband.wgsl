@group(0) @binding(0) var<storage, read> input: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> output: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read> taps: array<f32>;
@group(0) @binding(3) var<storage, read> twiddles: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read> params: array<u32>;
var<workgroup> polyphase: array<vec2<f32>, 25>;

@compute @workgroup_size(32)
fn filter_bank(@builtin(local_invocation_id) local: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>) {
    let frame = group.x;
    if (frame >= params[0]) { return; }
    if (local.x < 25u) {
        var sum = vec2<f32>(0.0);
        for (var tap = local.x; tap < 69u; tap += 25u) {
            sum += input[frame * 5u + 68u - tap] * taps[tap];
        }
        polyphase[local.x] = sum;
    }
    workgroupBarrier();
    if (local.x < 13u) {
        let bin = ((local.x + 19u) * 2u) % 25u;
        let rotation = ((params[1] + frame) % 5u) * 5u;
        var sum = vec2<f32>(0.0);
        for (var i = 0u; i < 25u; i += 1u) {
            let a = polyphase[(i + rotation) % 25u];
            let b = twiddles[(i * bin) % 25u];
            sum += vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
        }
        output[local.x * params[0] + frame] = sum;
    }
}

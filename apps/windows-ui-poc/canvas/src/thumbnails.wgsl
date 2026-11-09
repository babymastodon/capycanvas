struct Params { first: u32, count: u32, seconds: f32, unused: f32, }
@group(0) @binding(0) var<uniform> params: Params;
@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4(uv * vec2(2.0,-2.0) + vec2(-1.0,1.0),0.0,1.0);
}
@fragment fn fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let row = params.first + u32(pixel.y) / 32u;
    let uv = vec2(pixel.x, pixel.y % 32.0) / 32.0;
    let phase = f32(row % 97u) / 97.0 + params.seconds * 0.1;
    return vec4(fract(uv.x + phase), fract(uv.y + phase * 0.7), 0.2 + 0.6 * fract(phase + uv.x * uv.y),1.0);
}

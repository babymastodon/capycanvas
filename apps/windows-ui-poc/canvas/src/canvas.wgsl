struct Params {
    size: vec2<f32>,
    pen_count: u32,
    frame: u32,
}
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var document: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var<storage, read> dabs: array<vec4<f32>>;
@vertex fn fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 0.0, 1.0);
}
fn marker(p: vec2<f32>, origin: vec2<f32>, value: u32) -> vec4<f32> {
    if all(p >= origin) && all(p < origin + vec2(96.0, 4.0)) {
        let bit = u32((p.x - origin.x) / 4.0);
        return vec4(vec3(f32((value >> bit) & 1u)), 1.0);
    }
    if all(p >= origin - vec2(1.0)) && all(p < origin + vec2(97.0,5.0)) { return vec4(MARKER_GUARD_RGB,1.0); }
    return vec4(0.0);
}
@fragment fn initialize(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = p.xy / vec2(4096.0);
    return vec4(0.15 + 0.55 * uv.x, 0.18 + 0.5 * uv.y, 0.32, 1.0);
}
@fragment fn viewport(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let ink = marker(p.xy, vec2(4.0, params.size.y - 12.0), params.pen_count);
    if ink.a > 0.0 { return ink; }
    if all(p.xy >= vec2(195.0, 11.0)) && all(p.xy < vec2(211.0, 27.0)) { return vec4(0.0, 0.0, 0.0, 1.0); }
    let uv = p.xy / params.size;
    let stripes = select(0.0, 0.08, (u32(p.x) + params.frame * 2u) % 64u < 32u);
    return textureSample(document, linear_sampler, uv) + vec4(vec3(stripes), 0.0);
}
struct DabVertex { @builtin(position) position: vec4<f32>, @location(0) local: vec2<f32>, }
@vertex fn dab_vertex(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> DabVertex {
    var corners = array<vec2<f32>, 6>(vec2(-1.0,-1.0),vec2(1.0,-1.0),vec2(-1.0,1.0),vec2(-1.0,1.0),vec2(1.0,-1.0),vec2(1.0,1.0));
    let d = dabs[instance];
    let local = corners[vertex];
    let radius = (2.0 + d.z * 12.0) / params.size;
    return DabVertex(vec4((d.xy + local * radius) * vec2(2.0,-2.0) + vec2(-1.0,1.0), 0.0, 1.0), local);
}
@fragment fn dab_fragment(vertex: DabVertex) -> @location(0) vec4<f32> {
    if dot(vertex.local, vertex.local) > 1.0 { discard; }
    return vec4(0.75, 0.11, 0.16, 1.0);
}

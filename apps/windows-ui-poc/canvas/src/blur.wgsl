struct Blur { source_size: vec2<f32>, offset: f32, unused: f32, }
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, }
@vertex fn vertex(@builtin(vertex_index) i: u32) -> Vertex {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return Vertex(vec4(uv * vec2(2.0,-2.0) + vec2(-1.0,1.0), 0.0, 1.0), uv);
}
@fragment fn down(v: Vertex) -> @location(0) vec4<f32> {
    let d = blur.offset / blur.source_size;
    return (textureSample(source,linear_sampler,v.uv) * 4.0 + textureSample(source,linear_sampler,v.uv+d) + textureSample(source,linear_sampler,v.uv-d) + textureSample(source,linear_sampler,v.uv+vec2(d.x,-d.y)) + textureSample(source,linear_sampler,v.uv+vec2(-d.x,d.y))) / 8.0;
}
@fragment fn up(v: Vertex) -> @location(0) vec4<f32> {
    let d = blur.offset / blur.source_size;
    return (textureSample(source,linear_sampler,v.uv+vec2(-2.0*d.x,0.0)) + textureSample(source,linear_sampler,v.uv+vec2(-d.x,d.y))*2.0 + textureSample(source,linear_sampler,v.uv+vec2(0.0,2.0*d.y)) + textureSample(source,linear_sampler,v.uv+d)*2.0 + textureSample(source,linear_sampler,v.uv+vec2(2.0*d.x,0.0)) + textureSample(source,linear_sampler,v.uv+vec2(d.x,-d.y))*2.0 + textureSample(source,linear_sampler,v.uv+vec2(0.0,-2.0*d.y)) + textureSample(source,linear_sampler,v.uv-d)*2.0) / 12.0;
}

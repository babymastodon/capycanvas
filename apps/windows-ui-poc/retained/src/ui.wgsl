struct Frame{size:vec4<f32>,origin:vec4<f32>,clip:vec4<f32>,mask:vec4<f32>};
struct Item{rect:vec4<f32>,color:vec4<f32>,uv:vec4<f32>,params:vec4<f32>,border:vec4<f32>,frame:Frame};
@group(0) @binding(0) var<storage,read> items:array<Item>;
@group(1) @binding(0) var image:texture_2d<f32>;
@group(1) @binding(1) var image_sampler:sampler;
struct Vertex{@builtin(position) position:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) local:vec2<f32>,@location(2) @interpolate(flat) index:u32};
@vertex fn vs(@builtin(vertex_index) vertex:u32,@builtin(instance_index) index:u32)->Vertex{
 let corners=array<vec2<f32>,6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
 let item=items[index];let frame=item.frame;let unit=corners[vertex];let local=unit*(item.rect.zw+vec2((GLASS_GUTTER_PX+2.)*item.params.w,2.*item.params.w));let pixel=item.rect.xy+local;var out:Vertex;
 out.position=vec4(pixel/frame.size.xy*vec2(2.,-2.)+vec2(-1.,1.),0.,1.);out.uv=item.uv.xy+unit*item.uv.zw;out.local=local;out.index=index;return out;
}
fn inside(v:Vertex)->bool{let frame=items[v.index].frame;let p=v.position.xy+frame.origin.xy;return all(p>=frame.clip.xy)&&all(p<frame.clip.xy+frame.clip.zw);}
fn squircle_distance(local:vec2<f32>,size:vec2<f32>,requested_radius:f32)->f32{
 let radius=min(requested_radius,min(size.x,size.y)*0.5);let q=abs(local-size*0.5)-(size*0.5-vec2(radius));let outside=max(q,vec2(0.));
 return pow(pow(outside.x,4.)+pow(outside.y,4.),0.25)+min(max(q.x,q.y),0.)-radius;
}
fn distance(v:Vertex,item:Item)->f32{return squircle_distance(v.local,item.rect.zw,item.params.x);}
fn mask_alpha(v:Vertex)->f32{let frame=items[v.index].frame;return clamp(0.5-squircle_distance(v.position.xy+frame.origin.xy-frame.mask.xy,frame.mask.zw,frame.size.z),0.,1.);}
// Grayscale coverage: Copyright 2022-2025 Zed Industries, Inc.; Copyright Microsoft Corporation.
// Modified for Capy Canvas; Apache-2.0 and MIT terms in ../../THIRD_PARTY_NOTICES.md.
fn glyph_alpha(coverage:f32,color:vec3<f32>)->f32{
 let brightness=dot(color,vec3(0.30,0.59,0.11));let contrast=clamp(4.*(0.75-brightness),0.,1.);
 let alpha=coverage*(contrast+1.)/(coverage*contrast+1.);
 let ratios=vec4(0.1469*65536./65025.,-0.8911*256./255.,1.4644*65536./65025.,-0.3234*256./255.);
 return clamp(alpha+alpha*(1.-alpha)*((ratios.x*brightness+ratios.y)*alpha+ratios.z*brightness+ratios.w),0.,1.);
}
@fragment fn fs_rect(v:Vertex)->@location(0) vec4<f32>{
 if !inside(v){discard;}let item=items[v.index];let d=distance(v,item);let outer=clamp(0.5-d,0.,1.);let inner=clamp(0.5-d-item.params.y,0.,1.);
 return (item.border*(outer-inner)+item.color*inner)*mask_alpha(v);
}
@fragment fn fs_glyph(v:Vertex)->@location(0) vec4<f32>{if !inside(v){discard;}let item=items[v.index];let alpha=textureSample(image,image_sampler,v.uv).r;return item.color*(glyph_alpha(alpha,item.color.rgb/max(item.color.a,0.0001))*mask_alpha(v));}
@fragment fn fs_image(v:Vertex)->@location(0) vec4<f32>{if !inside(v){discard;}return textureSample(image,image_sampler,v.uv)*items[v.index].color*mask_alpha(v);}
@fragment fn fs_glass(v:Vertex)->@location(0) vec4<f32>{
 if !inside(v){discard;}let item=items[v.index];let opacity=item.color.a;
 if v.local.y>=item.rect.w{
  if v.local.y<item.rect.w+2. && v.local.x>=item.rect.z+GLASS_GUTTER_PX && v.local.x<item.rect.z+GLASS_GUTTER_PX+2.{return vec4(0.,opacity,opacity,opacity)*mask_alpha(v);}
  discard;
 }
 let frame=item.frame;let p=v.position.xy+frame.origin.xy;let background=textureSample(image,image_sampler,p/frame.origin.zw);let tint=item.border;
 return vec4(background.rgb*(1.-tint.a)+tint.rgb*tint.a,1.)*(clamp(0.5-distance(v,item),0.,1.)*opacity*mask_alpha(v));
}
@fragment fn fs_composite(v:Vertex)->@location(0) vec4<f32>{if !inside(v){discard;}return textureSample(image,image_sampler,v.uv)*items[v.index].color*mask_alpha(v);}

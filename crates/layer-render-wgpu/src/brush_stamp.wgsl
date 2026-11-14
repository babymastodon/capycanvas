struct Params { bounds: vec4<f32>, count: u32, shape_only: u32, strength: f32, unused: u32 }
struct Contacts { values: array<Dab,64> }
@group(0) @binding(0) var<uniform> style: Style;
@group(0) @binding(1) var<storage,read> contacts: Contacts;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage,read_write> output: array<u32>;

@compute @workgroup_size(64)
fn stamp(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x>=48u || id.y>=192u {return;}
    var packed=0u;
    for(var sample=0u;sample<4u;sample++) {
        let pixel=vec2<u32>(id.x*4u+sample,id.y);
        let world=mix(params.bounds.xy,params.bounds.zw,(vec2<f32>(pixel)+0.5)/192.0);
        let field=contact_field(world);
        var alpha=0.0;
        for(var i=0u;i<params.count;i++) {
            let dab=contacts.values[i];
            let coverage=brush_footprint(dab,world,field);
            let next=select(brush_alpha(dab,coverage),coverage*params.strength,params.shape_only!=0u);
            if contact_uniform() {alpha=max(alpha,next);}
            else {alpha+=(1.0-alpha)*next;}
        }
        packed|=u32(round(clamp(alpha*style.canvas_opacity.z,0.0,1.0)*255.0))<<(sample*8u);
    }
    output[id.y*48u+id.x]=packed;
}

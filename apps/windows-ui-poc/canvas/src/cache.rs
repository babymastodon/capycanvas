use std::sync::{Arc, atomic::{AtomicU64, Ordering}, mpsc::{self, Receiver, TryRecvError}};
use wgpu::util::DeviceExt;

pub use poc_common::THUMBNAIL_CAPACITY;
const THUMBNAIL_SIZE: u32 = 32;
const READBACK_STRIDE: u32 = 256;
const GLASS_CAPACITY: usize = 64;

pub struct ThumbnailReadback {
    pub row_ids: Vec<u32>,
    pub rgba: Vec<u8>,
}

pub struct GlassPaint { pub scale:f32, pub opacity:f32 }

enum MapState {
    Idle,
    Copied,
    Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

struct Readback {
    buffer: wgpu::Buffer,
    state: MapState,
    used: u64,
    tag: [u64; 2],
    ordinal: u64,
}

impl Readback {
    fn new(device: &wgpu::Device, size: u64, label: &'static str) -> Self {
        Self {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
            }),
            state: MapState::Idle, used: 0, tag: [0; 2], ordinal: 0,
        }
    }
    fn idle(&self) -> bool { matches!(self.state, MapState::Idle) }
    fn copied(&mut self, used: u64, tag: [u64; 2]) {
        self.used = used;
        self.tag = tag;
        self.state = MapState::Copied;
    }
    fn begin_map(&mut self) {
        if matches!(self.state, MapState::Copied) {
            let (sender, receiver) = mpsc::channel();
            self.buffer.slice(0..self.used).map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
            self.state = MapState::Mapping(receiver);
        }
    }
    fn finish_map(&mut self) -> Result<Option<Vec<u8>>, String> {
        let MapState::Mapping(receiver) = &self.state else { return Ok(None); };
        match receiver.try_recv() {
            Ok(Ok(())) => {
                let bytes = self.buffer.slice(0..self.used).get_mapped_range().map_err(|error| error.to_string())?.to_vec();
                self.buffer.unmap();
                self.state = MapState::Idle;
                Ok(Some(bytes))
            }
            Ok(Err(error)) => { self.state = MapState::Idle; Err(format!("GPU readback mapping failed: {error}")) }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => { self.state = MapState::Idle; Err("GPU readback callback disconnected".into()) }
        }
    }
}

pub struct Cache {
    pub blur_views: [wgpu::TextureView; 2],
    pub generation: Arc<AtomicU64>,
    pub thumbnail_view: wgpu::TextureView,
    dimensions: [u32; 2],
    scratch: [wgpu::TextureView; 3],
    blur_layout: wgpu::BindGroupLayout,
    blur_uniforms: [wgpu::Buffer; 4],
    blur_groups: [wgpu::BindGroup; 3],
    viewport_group: Option<(wgpu::TextureView, wgpu::BindGroup)>,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    thumbnail_texture: wgpu::Texture,
    thumbnail_uniform: wgpu::Buffer,
    thumbnail_ids: wgpu::Buffer,
    thumbnail_group: wgpu::BindGroup,
    thumbnails: wgpu::RenderPipeline,
    thumbnail_rows: Vec<u32>,
    readback_ids: [Vec<u32>;2],
    readbacks: [Readback; 2],
    readback_sequence: u64,
    readback_delivered: u64,
    static_copy_pending:bool,
    glass_uniform: wgpu::Buffer,
    glass_rects: wgpu::Buffer,
    glass_groups: [wgpu::BindGroup; 2],
    glass: wgpu::RenderPipeline,
}

impl Cache {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        assert!(width > 0 && height > 0);
        let extent = |divisor: u32| [width.div_ceil(divisor).max(1), height.div_ceil(divisor).max(1)];
        let blur_views = std::array::from_fn(|_| texture(device, "canvas blur cache", extent(4)).create_view(&Default::default()));
        let scratch = [extent(2), extent(4), extent(8)].map(|size| texture(device, "Kawase intermediate", size).create_view(&Default::default()));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("canvas cache sampler"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Kawase bindings"), entries: &[sampled_texture(0), sampled_sampler(1), uniform_entry(2)],
        });
        let blur_uniforms = [[width, height], extent(2), extent(4), extent(8)].map(|size| float_buffer(device, "Kawase size", &[size[0] as f32, size[1] as f32, 1.0, 0.0]));
        let group = |view: &wgpu::TextureView, buffer: &wgpu::Buffer| blur_group(device, &blur_layout, &sampler, view, buffer);
        let blur_groups = std::array::from_fn(|index| group(&scratch[index], &blur_uniforms[index + 1]));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("dual Kawase"), source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()) });
        let down = pipeline(device, &shader, "vertex", "down", &[&blur_layout],None);
        let up = pipeline(device, &shader, "vertex", "up", &[&blur_layout],None);
        let thumbnail_texture = texture(device, "visible row thumbnails", [THUMBNAIL_SIZE, THUMBNAIL_SIZE * THUMBNAIL_CAPACITY]);
        let thumbnail_view = thumbnail_texture.create_view(&Default::default());
        let thumbnail_uniform = word_buffer(device, "thumbnail rows", &[0; 4], wgpu::BufferUsages::UNIFORM);
        let thumbnail_ids=word_buffer(device,"thumbnail authored row IDs",&[0;THUMBNAIL_CAPACITY as usize],wgpu::BufferUsages::STORAGE);
        let thumbnail_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("thumbnail bindings"), entries: &[uniform_entry(0),wgpu::BindGroupLayoutEntry {binding:1,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer {ty:wgpu::BufferBindingType::Storage {read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None}] });
        let thumbnail_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("thumbnail rows"), layout: &thumbnail_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: thumbnail_uniform.as_entire_binding() },wgpu::BindGroupEntry {binding:1,resource:thumbnail_ids.as_entire_binding()}] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("thumbnail gradients"), source: wgpu::ShaderSource::Wgsl(include_str!("thumbnails.wgsl").into()) });
        let thumbnails = pipeline(device, &shader, "vertex", "fragment", &[&thumbnail_layout],None);
        let readbacks = std::array::from_fn(|_| Readback::new(device, (READBACK_STRIDE * THUMBNAIL_SIZE * THUMBNAIL_CAPACITY) as u64, "thumbnail readback"));
        let glass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("canvas glass bindings"), entries: &[sampled_texture(0), sampled_sampler(1), uniform_entry(2), wgpu::BindGroupLayoutEntry {
                binding: 3, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None,
            }],
        });
        let glass_uniform = float_buffer(device, "canvas glass size", &[width as f32, height as f32, 1.0, 0.0]);
        let glass_rects = word_buffer(device, "published glass rectangles", &[0; GLASS_CAPACITY * 4], wgpu::BufferUsages::STORAGE);
        let glass_groups = std::array::from_fn(|index| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas glass cache"), layout: &glass_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&blur_views[index]) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }, wgpu::BindGroupEntry { binding: 2, resource: glass_uniform.as_entire_binding() }, wgpu::BindGroupEntry { binding: 3, resource: glass_rects.as_entire_binding() }],
        }));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("canvas glass"), source: wgpu::ShaderSource::Wgsl(GLASS_SHADER.into()) });
        let glass = pipeline(device, &shader, "vertex", "fragment", &[&glass_layout],Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        Self {
            blur_views, generation: Arc::new(AtomicU64::new(0)), thumbnail_view, dimensions: [width, height], scratch, blur_layout, blur_uniforms, blur_groups,
            viewport_group: None, down, up, sampler, thumbnail_texture, thumbnail_uniform, thumbnail_ids, thumbnail_group, thumbnails, thumbnail_rows:Vec::new(),readback_ids:std::array::from_fn(|_|Vec::new()), readbacks, readback_sequence: 0, readback_delivered: 0,static_copy_pending:false,
            glass_uniform, glass_rects, glass_groups, glass,
        }
    }

    pub fn encode_blur(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, viewport: &wgpu::TextureView) -> u64 {
        if self.viewport_group.as_ref().is_none_or(|(view, _)| view != viewport) {
            self.viewport_group = Some((viewport.clone(), blur_group(device, &self.blur_layout, &self.sampler, viewport, &self.blur_uniforms[0])));
        }
        let next = self.generation.load(Ordering::Acquire).wrapping_add(1);
        let groups = [&self.viewport_group.as_ref().unwrap().1, &self.blur_groups[0], &self.blur_groups[1], &self.blur_groups[2]];
        let outputs = [&self.scratch[0], &self.scratch[1], &self.scratch[2], &self.blur_views[(next & 1) as usize]];
        for (index, (group, output)) in groups.into_iter().zip(outputs).enumerate() {
            let mut pass = render_pass(encoder, output, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(if index == 3 { &self.up } else { &self.down });
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
        next
    }

    pub fn publish_blur(&self, generation: u64) { self.generation.store(generation, Ordering::Release); }

    pub fn tick_thumbnails(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,row_ids:&[u32],seconds:f32,animated:bool) {
        let initial=self.thumbnail_rows.is_empty();
        let copied=animated && self.request_readback(device,encoder);
        self.encode_thumbnails(device,queue,encoder,row_ids,if animated {seconds}else{0.0});
        self.static_copy_pending=!row_ids.is_empty() && !copied && (initial || !animated);
        if self.static_copy_pending && self.request_readback(device,encoder) {self.static_copy_pending=false;}
    }
    pub fn has_pending_readback(&self)->bool {self.static_copy_pending || self.readbacks.iter().any(|slot|!slot.idle())}
    pub fn encode_thumbnails(&mut self, _device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, row_ids:&[u32], seconds: f32) {
        assert!(row_ids.len()<=THUMBNAIL_CAPACITY as usize && row_ids.iter().all(|row|*row<=65_535));
        if row_ids.is_empty() {self.thumbnail_rows.clear();return;}
        queue.write_buffer(&self.thumbnail_uniform,0,&words(&[row_ids.len() as u32,seconds.to_bits(),0,0]));
        queue.write_buffer(&self.thumbnail_ids,0,&words(row_ids));
        let mut pass = render_pass(encoder, &self.thumbnail_view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT));
        pass.set_pipeline(&self.thumbnails);pass.set_bind_group(0,&self.thumbnail_group,&[]);
        pass.set_viewport(0.0,0.0,THUMBNAIL_SIZE as f32,(THUMBNAIL_SIZE*row_ids.len() as u32) as f32,0.0,1.0);pass.draw(0..3,0..1);
        self.thumbnail_rows.clear();self.thumbnail_rows.extend_from_slice(row_ids);
    }
    pub fn thumbnail_rows(&self)->&[u32] {&self.thumbnail_rows}
    pub fn request_readback(&mut self,_device:&wgpu::Device,encoder:&mut wgpu::CommandEncoder)->bool {
        let count=self.thumbnail_rows.len() as u32;
        if count==0 {return false;}
        let Some(index)=self.readbacks.iter().position(Readback::idle) else{return false;};
        let slot=&mut self.readbacks[index];
        encoder.copy_texture_to_buffer(self.thumbnail_texture.as_image_copy(),wgpu::TexelCopyBufferInfo {buffer:&slot.buffer,layout:wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(READBACK_STRIDE),rows_per_image:Some(count*THUMBNAIL_SIZE)}},wgpu::Extent3d {width:THUMBNAIL_SIZE,height:count*THUMBNAIL_SIZE,depth_or_array_layers:1});
        self.readback_ids[index].clone_from(&self.thumbnail_rows);self.readback_sequence+=1;slot.ordinal=self.readback_sequence;
        slot.copied((READBACK_STRIDE*THUMBNAIL_SIZE*count) as u64,[0;2]);true
    }

    pub fn poll_readback(&mut self, device: &wgpu::Device,queue:&wgpu::Queue) -> Result<Option<ThumbnailReadback>, String> {
        for slot in &mut self.readbacks { slot.begin_map(); }
        device.poll(wgpu::PollType::Poll).map_err(|error| error.to_string())?;
        let mut latest = None;
        for (index,slot) in self.readbacks.iter_mut().enumerate() {
            if let Some(padded)=slot.finish_map()? && slot.ordinal>self.readback_delivered && latest.as_ref().is_none_or(|(ordinal,_)|*ordinal<slot.ordinal) {
                latest=Some((slot.ordinal,ThumbnailReadback {row_ids:std::mem::take(&mut self.readback_ids[index]),rgba:unpack_thumbnails(&padded)}));
            }
        }
        if self.static_copy_pending {
            let mut encoder=device.create_command_encoder(&Default::default());
            if self.request_readback(device,&mut encoder) {
                queue.submit([encoder.finish()]);self.static_copy_pending=false;
                for slot in &mut self.readbacks {slot.begin_map();}
            }
        }
        Ok(latest.map(|(ordinal, readback)| { self.readback_delivered = ordinal; readback }))
    }

    pub fn encode_glass(&self, _device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, rects: &[poc_common::GlassRect], paint: GlassPaint) {
        let GlassPaint {scale,opacity}=paint;
        assert!(rects.len() <= GLASS_CAPACITY && scale.is_finite() && scale > 0.0 && opacity.is_finite() && (0.0..=1.0).contains(&opacity));
        if opacity==0.0 || rects.is_empty() || self.generation.load(Ordering::Acquire) == 0 { return; }
        let rectangle_words: Vec<_> = rects.iter().flat_map(|rect| [rect.x.to_bits(), rect.y.to_bits(), rect.width.to_bits(), rect.height.to_bits()]).collect();
        queue.write_buffer(&self.glass_rects, 0, &words(&rectangle_words));
        queue.write_buffer(&self.glass_uniform, 0, &words(&[(self.dimensions[0] as f32).to_bits(), (self.dimensions[1] as f32).to_bits(), scale.to_bits(), opacity.to_bits()]));
        let mut pass = render_pass(encoder, target, wgpu::LoadOp::Load);
        pass.set_pipeline(&self.glass);
        pass.set_bind_group(0, &self.glass_groups[(self.generation.load(Ordering::Acquire) & 1) as usize], &[]);
        pass.draw(0..6, 0..rects.len() as u32);
    }
}

pub struct GpuTiming {
    pub frame: u64,
    pub qpc: u64,
    pub start_ticks: u64,
    pub end_ticks: u64,
    pub gpu_ms: f64,
}

pub struct GpuTimestamps {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readbacks: [Readback; 3],
}

impl GpuTimestamps {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(|| Self {
            queries: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("canvas GPU timestamps"), ty: wgpu::QueryType::Timestamp, count: 2 }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor { label: Some("canvas GPU timestamp resolve"), size: wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false }),
            readbacks: std::array::from_fn(|_| Readback::new(device, 16, "canvas GPU timestamps")),
        })
    }
    pub fn pass_writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites { query_set: &self.queries, beginning_of_pass_write_index: Some(0), end_of_pass_write_index: Some(1) }
    }
    pub fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder, frame: u64, qpc: u64) -> bool {
        let Some(slot) = self.readbacks.iter_mut().find(|slot| slot.idle()) else { return false; };
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &slot.buffer, 0, 16);
        slot.copied(16, [frame, qpc]);
        true
    }
    pub fn poll(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Vec<GpuTiming>, String> {
        for slot in &mut self.readbacks { slot.begin_map(); }
        device.poll(wgpu::PollType::Poll).map_err(|error| error.to_string())?;
        let mut timings = Vec::new();
        for slot in &mut self.readbacks {
            if let Some(bytes) = slot.finish_map()? {
                let start_ticks = u64::from_ne_bytes(bytes[0..8].try_into().unwrap());
                let end_ticks = u64::from_ne_bytes(bytes[8..16].try_into().unwrap());
                timings.push(GpuTiming { frame: slot.tag[0], qpc: slot.tag[1], start_ticks, end_ticks, gpu_ms: end_ticks.wrapping_sub(start_ticks) as f64 * queue.get_timestamp_period() as f64 / 1_000_000.0 });
            }
        }
        Ok(timings)
    }
}

fn texture(device: &wgpu::Device, label: &'static str, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[] })
}
fn words(values: &[u32]) -> Vec<u8> { values.iter().flat_map(|value| value.to_ne_bytes()).collect() }
fn word_buffer(device: &wgpu::Device, label: &'static str, values: &[u32], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: &words(values), usage: usage | wgpu::BufferUsages::COPY_DST })
}
fn float_buffer(device: &wgpu::Device, label: &'static str, values: &[f32]) -> wgpu::Buffer {
    word_buffer(device, label, &values.iter().map(|value| value.to_bits()).collect::<Vec<_>>(), wgpu::BufferUsages::UNIFORM)
}
fn sampled_texture(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }
}
fn sampled_sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}
fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }
}
fn blur_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, view: &wgpu::TextureView, uniform: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Kawase source"), layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) }, wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() }] })
}
fn pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, vertex: &str, fragment: &str, layouts: &[&wgpu::BindGroupLayout],blend:Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(fragment), bind_group_layouts: &layouts.iter().map(|layout| Some(*layout)).collect::<Vec<_>>(), immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some(fragment), layout: Some(&layout), vertex: wgpu::VertexState { module: shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] }, primitive: Default::default(), depth_stencil: None, multisample: Default::default(), fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(fragment), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend, write_mask: wgpu::ColorWrites::ALL })] }), multiview_mask: None, cache: None })
}
fn render_pass<'a>(encoder: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("canvas cache"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })], ..Default::default() })
}
fn unpack_thumbnails(padded: &[u8]) -> Vec<u8> { padded.as_chunks::<{ READBACK_STRIDE as usize }>().0.iter().flat_map(|row| row[..THUMBNAIL_SIZE as usize * 4].iter().copied()).collect() }

#[cfg(test)]
pub(crate) fn headless_instance()->wgpu::Instance {
    static INSTANCE:std::sync::OnceLock<wgpu::Instance>=std::sync::OnceLock::new();
    INSTANCE.get_or_init(||{let mut descriptor=wgpu::InstanceDescriptor::new_without_display_handle();descriptor.backends=wgpu::Backends::PRIMARY;descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);wgpu::Instance::new(descriptor)}).clone()
}

const GLASS_SHADER: &str = r#"
struct Params { size: vec2<f32>, scale: f32, opacity: f32, }
@group(0) @binding(0) var blurred: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read> rects: array<vec4<f32>>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) local: vec2<f32>, @location(1) @interpolate(flat) size: vec2<f32>, }
@vertex fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32) -> Vertex {
    var corners = array<vec2<f32>,6>(vec2(0.0,0.0),vec2(1.0,0.0),vec2(0.0,1.0),vec2(0.0,1.0),vec2(1.0,0.0),vec2(1.0,1.0));
    let rect = rects[instance] * params.scale;
    let local = corners[index] * (rect.zw + vec2(0.0,2.0));
    return Vertex(vec4((rect.xy + local) / params.size * vec2(2.0,-2.0) + vec2(-1.0,1.0),0.0,1.0),local,rect.zw);
}
@fragment fn fragment(v: Vertex) -> @location(0) vec4<f32> {
    if v.local.y >= v.size.y {
        if v.local.y < v.size.y+2.0 && v.local.x >= v.size.x/3.0 && v.local.x < 2.0*v.size.x/3.0 {return vec4(0.0,params.opacity,params.opacity,params.opacity);}
        discard;
    }
    let radius = min(12.0 * params.scale, min(v.size.x,v.size.y)*0.5);
    let q = abs(v.local - v.size*0.5) - (v.size*0.5 - vec2(radius));
    if length(max(q,vec2(0.0))) + min(max(q.x,q.y),0.0) - radius > 0.0 { discard; }
    let backdrop = textureSample(blurred,linear_sampler,v.position.xy/params.size);
    return vec4(backdrop.rgb*params.opacity,params.opacity);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_renders_and_reads_the_previous_tick_without_a_window() {
        let instance = headless_instance();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let supported = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: supported, ..Default::default() })).unwrap();
        let mut cache = Cache::new(&device, 64, 64);
        let viewport = texture(&device, "cache test viewport", [64, 64]).create_view(&Default::default());
        let mut timestamps = GpuTimestamps::new(&device);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &viewport, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })],
                timestamp_writes: timestamps.as_ref().map(GpuTimestamps::pass_writes), ..Default::default()
            });
        }
        let generation = cache.encode_blur(&device, &queue, &mut encoder, &viewport);
        assert_eq!(cache.generation.load(Ordering::Acquire), 0);
        cache.encode_thumbnails(&device, &queue, &mut encoder, &[700,701,702], 0.0);
        if let Some(timestamps) = &mut timestamps { assert!(timestamps.resolve(&mut encoder, 19, 123)); }
        queue.submit([encoder.finish()]);
        cache.publish_blur(generation);
        assert_eq!(generation, 1);
        assert_eq!(cache.generation.load(Ordering::Acquire), 1);
        let mut encoder = device.create_command_encoder(&Default::default());
        assert!(cache.request_readback(&device, &mut encoder));
        cache.encode_thumbnails(&device, &queue, &mut encoder, &[900,901,902], 2.0);
        cache.encode_glass(&device, &queue, &mut encoder, &viewport, &[poc_common::GlassRect { x: 4.0, y: 4.0, width: 40.0, height: 40.0, revision: 0 }], GlassPaint {scale:1.0,opacity:1.0});
        queue.submit([encoder.finish()]);
        let early = cache.poll_readback(&device,&queue).unwrap();
        let early_timing = timestamps.as_mut().map(|timestamps| timestamps.poll(&device, &queue).unwrap()).unwrap_or_default();
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let readback = early.or_else(|| cache.poll_readback(&device,&queue).unwrap()).expect("previous tick thumbnail readback");
        assert_eq!(readback.row_ids,[700,701,702]);
        assert_eq!(readback.rgba.len(), 3 * 32 * 32 * 4);
        let phase = (700 % 97) as f32 / 97.0;
        let expected_red = ((0.5 / 32.0 + phase).fract() * 255.0).round() as i16;
        assert!((readback.rgba[0] as i16 - expected_red).abs() <= 1);
        assert!(readback.rgba.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 255));
        if let Some(timestamps) = &mut timestamps {
            let timings = if early_timing.is_empty() { timestamps.poll(&device, &queue).unwrap() } else { early_timing };
            assert_eq!(timings.len(), 1);
            assert_eq!((timings[0].frame, timings[0].qpc), (19, 123));
            assert!(timings[0].gpu_ms.is_finite());
        }
    }
    #[test]
    fn sparse_thumbnail_ids_preserve_previous_tick_metadata_and_pixels() {
        let instance = headless_instance();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut cache = Cache::new(&device, 64, 64);
        let mut encoder = device.create_command_encoder(&Default::default());
        cache.encode_thumbnails(&device, &queue, &mut encoder, &[0,70,140], 0.0);
        queue.submit([encoder.finish()]);
        let mut previous_seconds=0.0;
        for (tick,next) in [[70,140,210],[140,210,280],[210,280,350]].into_iter().enumerate() {
            let previous=cache.thumbnail_rows().to_vec();
            let mut encoder=device.create_command_encoder(&Default::default());
            cache.tick_thumbnails(&device,&queue,&mut encoder,&next,tick as f32+1.0,true);
            queue.submit([encoder.finish()]);
            let ready=cache.poll_readback(&device,&queue).unwrap();
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let readback=ready.or_else(||cache.poll_readback(&device,&queue).unwrap()).expect("previous sparse atlas");
            assert_eq!(readback.row_ids,previous);assert_eq!(cache.thumbnail_rows(),&next);
            assert_eq!(readback.rgba.len(),previous.len()*32*32*4);
            for (slot,row) in previous.into_iter().enumerate() {
                let phase=(row%97) as f32/97.0+previous_seconds*0.1;
                let expected=[((0.5/32.0+phase).fract()*255.0).round(),((0.5/32.0+phase*0.7).fract()*255.0).round()];
                let offset=slot*32*32*4;
                for (channel,value) in expected.into_iter().enumerate() {assert!((readback.rgba[offset+channel] as f32-value).abs()<=1.0,"authored row {row} was overwritten or seeded from its atlas slot");}
                assert_eq!(readback.rgba[offset+3],255);
            }
            previous_seconds=tick as f32+1.0;
        }
    }
    #[test]
    fn static_thumbnails_read_sparse_rows_at_time_zero_then_stop_polling() {
        let instance=headless_instance();
        let adapter=pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device,queue)=pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut cache=Cache::new(&device,64,64);
        for rows in [[0,70,140],[140,210,280]] {
            assert!(!cache.has_pending_readback());
            let mut encoder=device.create_command_encoder(&Default::default());cache.tick_thumbnails(&device,&queue,&mut encoder,&rows,99.0,false);queue.submit([encoder.finish()]);
            assert!(cache.has_pending_readback());let early=cache.poll_readback(&device,&queue).unwrap();device.poll(wgpu::PollType::wait_indefinitely()).unwrap();let readback=early.or_else(||cache.poll_readback(&device,&queue).unwrap()).unwrap();
            assert_eq!(readback.row_ids,rows);assert!(!cache.has_pending_readback());
            let phase=(rows[0]%97) as f32/97.0;let expected=((0.5/32.0+phase).fract()*255.0).round();assert!((readback.rgba[0] as f32-expected).abs()<=1.0);
        }
    }
    #[test]
    fn glass_scales_dips_once_and_fades_two_physical_pixel_cyan_over_opaque_canvas() {
        let instance=headless_instance();
        let adapter=pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device,queue)=pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let mut cache=Cache::new(&device,128,128);
        let backdrop=texture(&device,"white blur source",[128,128]);let target=texture(&device,"raw opaque canvas",[128,128]);
        let mut encoder=device.create_command_encoder(&Default::default());
        drop(render_pass(&mut encoder,&backdrop.create_view(&Default::default()),wgpu::LoadOp::Clear(wgpu::Color::WHITE)));
        let generation=cache.encode_blur(&device,&queue,&mut encoder,&backdrop.create_view(&Default::default()));queue.submit([encoder.finish()]);cache.publish_blur(generation);
        for opacity in [1.0f32,0.5] {
            let mut encoder=device.create_command_encoder(&Default::default());
            drop(render_pass(&mut encoder,&target.create_view(&Default::default()),wgpu::LoadOp::Clear(wgpu::Color::BLACK)));
            cache.encode_glass(&device,&queue,&mut encoder,&target.create_view(&Default::default()),&[poc_common::GlassRect{x:8.0,y:8.0,width:40.0,height:32.0,revision:0}],GlassPaint {scale:1.5,opacity});
            let readback=device.create_buffer(&wgpu::BufferDescriptor {label:None,size:128*128*4,usage:wgpu::BufferUsages::COPY_DST|wgpu::BufferUsages::MAP_READ,mapped_at_creation:false});
            encoder.copy_texture_to_buffer(target.as_image_copy(),wgpu::TexelCopyBufferInfo {buffer:&readback,layout:wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(512),rows_per_image:Some(128)}},target.size());queue.submit([encoder.finish()]);
            let (tx,rx)=std::sync::mpsc::channel();readback.slice(..).map_async(wgpu::MapMode::Read,move |result|{let _=tx.send(result);});device.poll(wgpu::PollType::wait_indefinitely()).unwrap();rx.recv().unwrap().unwrap();
            let bytes=readback.slice(..).get_mapped_range().unwrap();let pixel=|x:usize,y:usize|&bytes[(y*128+x)*4..(y*128+x+1)*4];let intensity=(255.0*opacity).round() as u8;
            for channel in 0..3 {assert!((pixel(60,30)[channel] as i16-intensity as i16).abs()<=1);}
            assert_eq!(pixel(60,30)[3],255);
            assert_eq!(pixel(10,30),&[0,0,0,255]);assert_eq!(pixel(42,62),&[0,0,0,255]);
            for channel in 0..3 {assert!((pixel(42,57)[channel] as i16-intensity as i16).abs()<=1);}
            let cyan_rows:Vec<_>=(0..128).filter(|y|{let rgba=pixel(42,*y);rgba[0]==0 && rgba[3]==255 && (rgba[1] as i16-intensity as i16).abs()<=1 && (rgba[2] as i16-intensity as i16).abs()<=1}).collect();assert_eq!(cyan_rows,[60,61]);
        }
    }
    #[test]
    fn thumbnail_unpack_preserves_row_order_and_discards_only_padding() {
        let mut padded = vec![233; (READBACK_STRIDE * THUMBNAIL_SIZE * 2) as usize];
        for (index, row) in padded.as_chunks_mut::<{ READBACK_STRIDE as usize }>().0.iter_mut().enumerate() {
            for pixel in row[..128].as_chunks_mut::<4>().0.iter_mut() { pixel.copy_from_slice(&[index as u8, 2, 3, 255]); }
        }
        let rgba = unpack_thumbnails(&padded);
        assert_eq!(rgba.len(), 32 * 32 * 2 * 4);
        assert_eq!(&rgba[..4], &[0, 2, 3, 255]);
        assert_eq!(&rgba[32 * 32 * 4..32 * 32 * 4 + 4], &[32, 2, 3, 255]);
        assert_eq!(&rgba[rgba.len()-4..], &[63, 2, 3, 255]);
    }
}

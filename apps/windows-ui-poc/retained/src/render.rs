use crate::{scene::*, text::GlyphUpload};
use std::{sync::Arc,collections::{HashMap,HashSet},cell::RefCell,ops::Range};
#[cfg(windows)]
use {
    crossbeam_channel::{Receiver, Sender},
    poc_canvas::{Canvas, FrameSlot},
    windows::{
        Win32::{
            Foundation::*,
            Graphics::{DirectComposition::*, Dwm::DwmFlush, Gdi::ClientToScreen},
            System::{
                Com::{COINIT_MULTITHREADED, CoInitializeEx},
                LibraryLoader::{GetModuleHandleW, GetProcAddress},
            },
            UI::{Input::Pointer::GetPointerInfo, WindowsAndMessaging::*},
        },
        core::{Interface, w},
    },
};
#[cfg(windows)]
use {
    poc_common::AppOptions,
    std::{
        sync::OnceLock,
        time::{Duration, Instant},
    },
};

pub struct Transaction {
    pub layers: Vec<Layer>,
    pub uploads: Vec<GlyphUpload>,
    pub state: u16,
    pub geometry: Geometry,
    pub rows: std::ops::Range<u32>,
    pub visible_rows: u32,
    pub motion: Arc<Motion>,
}
pub enum Event {
    Wheel(f32),
    Touch(u32, f32, u8, i64),
    CancelTouch(u32),
    Drawer(bool),
    Zen(bool),
    ScrollTo(f32),
    Popup(Option<Popup>),
    Size(u32, u32, f32),
    Shutdown,
}
#[derive(Clone)]
pub struct Popup {
    pub rect: Rect,
    pub layer: Layer,
    pub uploads: Vec<GlyphUpload>,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rect: [f32; 4],
    color: [f32; 4],
    uv: [f32; 4],
    params: [f32; 4],
    border: [f32; 4],
    frame: Frame,
}
impl Instance {
    fn new(rect: Rect) -> Self {
        Self {
            rect: rect.array(),
            color: [1.0; 4],
            uv: [0.0, 0.0, 1.0, 1.0],
            params: [0.0; 4],
            border: [0.0; 4],
            frame: Frame::default(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Frame {
    size: [f32; 4],
    origin: [f32; 4],
    clip: [f32; 4],
    mask: [f32; 4],
}
struct Cached {
    layer: Layer,
    texture: wgpu::Texture,
}
#[derive(Clone, Copy)]
struct SurfacePaint<'a> {
    view: &'a wgpu::TextureView,
    frame: Frame,
}
struct Composition {geometry:Geometry,drawer:f32,scroll:f32}
#[derive(Clone,Copy)]
struct Placement {
    rect: Rect,
    scale: f32,
    opacity: f32,
}
struct Batch {
    pipeline: usize,
    source: wgpu::TextureView,
    instances: Range<u32>,
}
struct TargetPass {
    view: wgpu::TextureView,
    clear: bool,
    batches: Vec<Batch>,
}
#[derive(Default)]
struct FrameItems {
    instances: Vec<Instance>,
    targets: Vec<TargetPass>,
    live_textures: HashSet<wgpu::Texture>,
}
impl FrameItems {
    fn target(&mut self, view: &wgpu::TextureView, clear: bool) {
        if clear || self.targets.last().is_none_or(|target| target.view != *view) {
            self.targets.push(TargetPass { view: view.clone(), clear, batches: Vec::new() });
        }
    }
    fn draw(&mut self, target: SurfacePaint<'_>, instances: &[Instance], source: &wgpu::TextureView, pipeline: usize) {
        if instances.is_empty() { return; }
        self.target(target.view, false);
        let first = self.instances.len() as u32;
        self.instances.extend(instances.iter().map(|item| Instance { frame: target.frame, ..*item }));
        self.targets.last_mut().unwrap().batches.push(Batch { pipeline, source: source.clone(), instances: first..self.instances.len() as u32 });
    }
}
struct InstanceBuffer { buffer: wgpu::Buffer, capacity: u64, group: wgpu::BindGroup }
struct Paint {
    layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 5],
    sampler: wgpu::Sampler,
    glyphs: wgpu::Texture,
    white: wgpu::Texture,
    icons: wgpu::Texture,
    icon_scale: f32,
    tint: [f32; 4],
    frame: RefCell<FrameItems>,
    buffer: RefCell<Option<InstanceBuffer>>,
    sources: RefCell<HashMap<wgpu::Texture,wgpu::BindGroup>>,
}
fn layer_extent(value: f32) -> u32 { (value.ceil().max(1.0) as u32).div_ceil(64) * 64 }
fn composite_uv(width: f32, height: f32, texture: &wgpu::Texture) -> [f32;4] {
    [0.0, 0.0, width / texture.width() as f32, height / texture.height() as f32]
}
fn texture(
    device: &wgpu::Device,
    label: &str,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w.max(1),
            height: h.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn icon_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scale: f32,
) -> Result<wgpu::Texture, String> {
    let tree = resvg::usvg::Tree::from_data(
        include_bytes!("../../winui/Assets/layer-eye-symbolic.svg"),
        &resvg::usvg::Options::default(),
    )
    .map_err(|e| e.to_string())?;
    let size = (16.0 * scale).ceil().max(1.0) as u32;
    let mut pixels = resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon allocation failed")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(size as f32 / 16.0, size as f32 / 16.0),
        &mut pixels.as_mut(),
    );
    let icons = texture(
        device,
        "shared SVG eye atlas",
        size,
        size,
        wgpu::TextureFormat::Rgba8Unorm,
    );
    queue.write_texture(
        icons.as_image_copy(),
        pixels.data(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: None,
        },
        icons.size(),
    );
    Ok(icons)
}
#[cfg(test)]
fn clear(encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear retained surface"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    }));
}
impl Paint {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, scale: f32) -> Result<Self, String> {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame instances"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("retained texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("retained"),
            bind_group_layouts: &[Some(&layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("retained five pipelines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ui.wgsl").replace("GLASS_GUTTER_PX",&format!("{:.1}",poc_common::GLASS_GUTTER_PX)).into()),
        });
        let pipelines = [
            "fs_rect",
            "fs_glyph",
            "fs_image",
            "fs_glass",
            "fs_composite",
        ]
        .map(|fragment| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fragment),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let glyphs = texture(
            device,
            "DirectWrite R8 glyph atlas",
            crate::text::ATLAS_SIZE,
            crate::text::ATLAS_SIZE,
            wgpu::TextureFormat::R8Unorm,
        );
        let white = texture(device, "white", 1, 1, wgpu::TextureFormat::Rgba8Unorm);
        queue.write_texture(
            white.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: None,
            },
            white.size(),
        );
        let icons = icon_texture(device, queue, scale)?;
        let tint = std::array::from_fn(|index| {
            poc_common::specification()["glass_tint"][index]
                .as_u64()
                .expect("shared tint") as f32
                / 255.0
        });
        Ok(Self {
            layout,
            image_layout,
            frame: RefCell::default(),
            buffer: RefCell::default(),
            sources: RefCell::default(),
            pipelines,
            sampler,
            glyphs,
            white,
            icons,
            icon_scale: scale,
            tint,
        })
    }
    fn upload(&self, queue: &wgpu::Queue, uploads: impl IntoIterator<Item = GlyphUpload>) {
        for upload in uploads {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.glyphs,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: upload.origin[0],
                        y: upload.origin[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.size[0]),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: upload.size[0],
                    height: upload.size[1],
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    fn clear(&self, view: &wgpu::TextureView) { self.frame.borrow_mut().target(view, true); }
    fn draw(&self, _device: &wgpu::Device, _encoder: &mut wgpu::CommandEncoder, target: SurfacePaint<'_>, instances: &[Instance], source: &wgpu::TextureView, pipeline: usize) {
        self.frame.borrow_mut().draw(target, instances, source, pipeline);
    }
    fn flush(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        let mut frame = self.frame.borrow_mut();
        if frame.instances.is_empty() { return; }
        let bytes = bytemuck::cast_slice(&frame.instances);
        let mut storage = self.buffer.borrow_mut();
        if storage.as_ref().is_none_or(|storage| storage.capacity < bytes.len() as u64) {
            let capacity = (bytes.len() as u64).next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("retained frame instances"), size: capacity, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("retained frame instances"), layout: &self.layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }] });
            *storage = Some(InstanceBuffer { buffer, capacity, group });
        }
        let storage = storage.as_ref().unwrap();
        queue.write_buffer(&storage.buffer, 0, bytes);
        let mut sources = self.sources.borrow_mut();
        sources.retain(|texture, _| *texture == self.white || *texture == self.glyphs || *texture == self.icons || frame.live_textures.contains(texture) || frame.targets.iter().flat_map(|target| &target.batches).any(|batch| batch.source.texture() == texture));
        for batch in frame.targets.iter().flat_map(|target| &target.batches) {
            sources.entry(batch.source.texture().clone()).or_insert_with(|| device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("retained source texture"), layout: &self.image_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&batch.source) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
            }));
        }
        for target in &frame.targets {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("retained layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target.view, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: if target.clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store } })],
                ..Default::default()
            });
            pass.set_bind_group(0, &storage.group, &[]);
            for batch in &target.batches {
                pass.set_pipeline(&self.pipelines[batch.pipeline]);
                pass.set_bind_group(1, &sources[batch.source.texture()], &[]);
                pass.draw(0..6, batch.instances.clone());
            }
        }
        frame.instances.clear();
        frame.targets.clear();
        frame.live_textures.clear();
    }
    fn composite_group(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: SurfacePaint<'_>,
        source: &wgpu::TextureView,
        opacity: f32,
    ) {
        let mut group = Instance::new(Rect::new(
            0.0,
            0.0,
            target.frame.size[0],
            target.frame.size[1],
        ));
        group.color = [opacity; 4];
        self.draw(device, encoder, target, &[group], source, 4);
    }
    fn record(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        cached: &Cached,
        geometry: Geometry,
    ) {
        let scale = geometry.scale;
        let base = cached.layer.rect;
        let bounds = geometry.physical(base).array();
        let frame = Frame {
            size: [
                cached.texture.width() as f32,
                cached.texture.height() as f32,
                0.0,
                0.0,
            ],
            origin: [
                bounds[0],
                bounds[1],
                geometry.width * scale,
                geometry.height * scale,
            ],
            clip: bounds,
            mask: bounds,
        };
        let target = cached.texture.create_view(&Default::default());
        self.clear(&target);
        let sources = [
            self.white.create_view(&Default::default()),
            self.glyphs.create_view(&Default::default()),
            self.icons.create_view(&Default::default()),
        ];
        let mut batches: [Vec<Instance>; 3] = Default::default();
        for primitive in cached.layer.display.iter() {
            let (rect, index, mut item) = match primitive {
                Prim::Rect {
                    rect,
                    color,
                    radius,
                    border,
                    border_color,
                } => {
                    let mut item = Instance::new(*rect);
                    item.color = *color;
                    item.params = [radius * scale, border * scale, 0.0, 0.0];
                    item.border = *border_color;
                    (*rect, 0, item)
                }
                Prim::Glyph { rect, uv, color } => {
                    let mut item = Instance::new(*rect);
                    item.color = *color;
                    item.uv = *uv;
                    (*rect, 1, item)
                }
                Prim::Image { rect, uv, atlas: 0 } => {
                    let mut item = Instance::new(*rect);
                    item.uv = *uv;
                    (*rect, 2, item)
                }
                _ => continue,
            };
            item.rect = [
                (rect.x - base.x + cached.layer.origin[0]) * scale,
                (rect.y - base.y + cached.layer.origin[1]) * scale,
                rect.w * scale,
                rect.h * scale,
            ];
            batches[index].push(item);
        }
        for index in [0, 2, 1] {
            self.draw(device, encoder, SurfacePaint { view: &target, frame }, &batches[index], &sources[index], index);
        }
    }

    fn compose_layers(&self,device:&wgpu::Device,encoder:&mut wgpu::CommandEncoder,target:SurfacePaint<'_>,layers:&HashMap<LayerId,Cached>,motion:Composition,snapshot:Option<&poc_canvas::cache::CacheSnapshot>){
        let geometry=motion.geometry;
        for probes in [false,true] {
        let order=if probes {[LayerId::Drawer,LayerId::List,LayerId::Layers,LayerId::Properties,LayerId::Title]}else{[LayerId::Drawer,LayerId::Layers,LayerId::List,LayerId::Properties,LayerId::Title]};
        for id in order {
            let Some(cached) = layers.get(&id) else {
                continue;
            };
            let mut rect = cached.layer.rect;
            let mut info = target.frame;
            info.clip = geometry.physical(cached.layer.clip).array();
            if id == LayerId::Drawer {
                rect.x += motion.drawer;
                info.clip[0] += motion.drawer * geometry.scale;
            }
            if id == LayerId::List {
                rect.y -= motion.scroll;
                info.clip = geometry.physical(geometry.list).array();
                info.mask = geometry.physical(geometry.layers).array();
                info.size[2] = metric("radius") * geometry.scale;
            }
            let target=SurfacePaint{view:target.view,frame:info};let placement=Placement{rect,scale:geometry.scale,opacity:1.0};
            if probes {self.compose_probes(device,encoder,target,cached,placement);}
            else {self.compose_content(device,encoder,target,cached,placement,snapshot);}
        }
        }
    }
    fn compose(&self,device:&wgpu::Device,encoder:&mut wgpu::CommandEncoder,target:SurfacePaint<'_>,cached:&Cached,placement:Placement,snapshot:Option<&poc_canvas::cache::CacheSnapshot>){
        self.compose_content(device,encoder,target,cached,placement,snapshot);
        self.compose_probes(device,encoder,target,cached,placement);
    }
    fn compose_content(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: SurfacePaint<'_>,
        cached: &Cached,
        placement: Placement,
        snapshot: Option<&poc_canvas::cache::CacheSnapshot>,
    ) {
        let Placement {
            rect: placement,
            scale,
            opacity,
        } = placement;
        let delta = [
            placement.x - cached.layer.rect.x,
            placement.y - cached.layer.rect.y,
        ];
        if let Some(snapshot) = snapshot {
            self.frame.borrow_mut().live_textures.extend(snapshot.blur_views.iter().chain([&snapshot.thumbnail_view]).map(|view| view.texture().clone()));
        }
        if let Some(blur) = snapshot.and_then(|s| s.blur_view()) {
            let glass: Vec<_> = cached
                .layer
                .display
                .iter()
                .filter_map(|primitive| {
                    if let Prim::Glass { rect, radius } = primitive {
                        let mut item = Instance::new(Rect::new(
                            (rect.x + delta[0]) * scale,
                            (rect.y + delta[1]) * scale,
                            rect.w * scale,
                            rect.h * scale,
                        ));
                        item.color = [opacity; 4];
                        item.params = [radius * scale, 0.0, 0.0, 1.0];
                        item.border = self.tint;
                        Some(item)
                    } else {
                        None
                    }
                })
                .collect();
            let mut outer=target;outer.frame.clip[2]+=poc_common::GLASS_GUTTER_PX+2.0;outer.frame.clip[3]+=2.0;
            self.draw(device, encoder, outer, &glass, blur, 3);
        }
        let mut item = Instance::new(Rect::new(
            placement.x * scale,
            placement.y * scale,
            placement.w * scale,
            placement.h * scale,
        ));
        item.color = [opacity; 4];
        item.uv = composite_uv(placement.w * scale, placement.h * scale, &cached.texture);
        self.draw(
            device,
            encoder,
            target,
            &[item],
            &cached.texture.create_view(&Default::default()),
            4,
        );
        if let Some(snapshot) = snapshot {
            let images: Vec<_> = cached
                .layer
                .display
                .iter()
                .filter_map(|primitive| {
                    let Prim::Image { rect, atlas, .. } = primitive else {
                        return None;
                    };
                    if *atlas == 0 {
                        return None;
                    }
                    let slot = snapshot
                        .thumbnail_rows
                        .iter()
                        .position(|row| *row == *atlas - 1)?;
                    let mut item = Instance::new(Rect::new(
                        (rect.x + delta[0]) * scale,
                        (rect.y + delta[1]) * scale,
                        rect.w * scale,
                        rect.h * scale,
                    ));
                    item.color = [opacity; 4];
                    item.uv = [
                        0.0,
                        slot as f32 / poc_common::THUMBNAIL_CAPACITY as f32,
                        1.0,
                        1.0 / poc_common::THUMBNAIL_CAPACITY as f32,
                    ];
                    Some(item)
                })
                .collect();
            self.draw(
                device,
                encoder,
                target,
                &images,
                &snapshot.thumbnail_view,
                2,
            );
        }
    }
    fn compose_probes(&self,device:&wgpu::Device,encoder:&mut wgpu::CommandEncoder,target:SurfacePaint<'_>,cached:&Cached,placement:Placement){
        let Placement{rect:placement,scale,opacity}=placement;
        let delta=[placement.x-cached.layer.rect.x,placement.y-cached.layer.rect.y];
        let probes: Vec<_> = cached
            .layer
            .display
            .iter()
            .filter_map(|primitive| {
                if let Prim::Probe { rect, color } = primitive {
                    let mut item = Instance::new(Rect::new(
                        ((rect.x + delta[0]) * scale).round(),
                        ((rect.y + delta[1]) * scale).round(),
                        (rect.w * scale).round(),
                        (rect.h * scale).round(),
                    ));
                    item.color = color.map(|c| c * opacity);
                    Some(item)
                } else {
                    None
                }
            })
            .collect();
        self.draw(
            device,
            encoder,
            target,
            &probes,
            &self.white.create_view(&Default::default()),
            0,
        );
    }
}

#[cfg(windows)]
type Clock = unsafe extern "system" fn(u32, *const HANDLE, u32) -> u32;
#[cfg(windows)]
fn clock_wait() -> bool {
    static CLOCK: OnceLock<Option<Clock>> = OnceLock::new();
    static TIMEOUT: OnceLock<u32> = OnceLock::new();
    let timeout = TIMEOUT.get_or_init(|| {
        let mut info = windows::Win32::Graphics::Dwm::DWM_TIMING_INFO {
            cbSize: std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_TIMING_INFO>() as u32,
            ..Default::default()
        };
        let mut frequency = 0;
        unsafe {
            if windows::Win32::Graphics::Dwm::DwmGetCompositionTimingInfo(HWND::default(), &mut info).is_ok()
                && windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
                    .is_ok()
                && frequency > 0
                && info.qpcRefreshPeriod > 0
            {
                return (info.qpcRefreshPeriod * 1000)
                    .div_ceil(frequency as u64)
                    .max(1) as u32;
            }
        }
        17
    });
    let clock = CLOCK.get_or_init(|| unsafe {
        GetModuleHandleW(w!("dcomp.dll"))
            .ok()
            .and_then(|module| {
                GetProcAddress(
                    module,
                    windows::core::s!("DCompositionWaitForCompositorClock"),
                )
            })
            .map(|proc| std::mem::transmute::<unsafe extern "system" fn() -> isize, Clock>(proc))
    });
    unsafe {
        if let Some(clock) = clock {
            if clock(0,std::ptr::null(),*timeout)==WAIT_FAILED.0{let _=DwmFlush();false}else{true}
        } else {
            let _ = DwmFlush();
            false
        }
    }
}
#[cfg(windows)]
fn trace(canvas: &Canvas, kind: &str, mut value: serde_json::Value) {
    value["kind"] = kind.into();
    value["qpc"] = crate::native::qpc().into();
    let _ = canvas.trace(&value);
}
#[cfg(windows)]
fn config(width: u32, height: u32) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Rgba8Unorm,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 1,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
        view_formats: vec![],
        color_space: Default::default(),
    }
}
#[cfg(windows)]
fn acquire(surface: &wgpu::Surface<'_>) -> Result<wgpu::SurfaceTexture, String> {
    match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
        error => Err(format!("retained surface acquire: {error:?}")),
    }
}
#[cfg(windows)]
struct PopupState {
    ui: Sender<crate::native::UiEvent>,
    scale: f32,
}
#[cfg(windows)]
unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
    }
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const PopupState).as_ref() };
    match message {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_POINTERDOWN | WM_POINTERUPDATE => return LRESULT(0),
        WM_LBUTTONUP | WM_POINTERUP => {
            if let Some(state) = state {
                let mut y = (lparam.0 >> 16) as i16 as f32;
                if message == WM_POINTERUP {
                    let mut info = windows::Win32::UI::Input::Pointer::POINTER_INFO::default();
                    if unsafe { GetPointerInfo((wparam.0 & 0xffff) as u32, &mut info) }.is_ok() {
                        let mut point = info.ptPixelLocation;
                        unsafe {
                            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
                        }
                        y = point.y as f32;
                    }
                }
                if let Some(index) = popup_item_hit(y,state.scale) {
                    let _ = state.ui.try_send(crate::native::UiEvent::Pointer(
                        Target::MenuItem(index),
                        0.0,
                        0.0,
                        2,crate::native::qpc(),
                    ));
                }
                let _ = state.ui.try_send(crate::native::UiEvent::ClosePopup);
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_NCDESTROY => unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        },
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
#[cfg(windows)]
struct PopupWindow {
    hwnd: HWND,
    _state: Box<PopupState>,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    surface: wgpu::Surface<'static>,
    frame_slot: FrameSlot,
    cached: Cached,
    chrome: wgpu::Texture,
    rect: Rect,
}
#[cfg(windows)]
impl Drop for PopupWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
#[cfg(windows)]
impl PopupWindow {
    fn new(
        owner: HWND,
        popup: Popup,
        geometry: Geometry,
        shared: &poc_canvas::SharedGpu,
        device: &IDCompositionDevice,
        ui: Sender<crate::native::UiEvent>,
    ) -> Result<Self, String> {
        let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: module.into(),
            lpszClassName: w!("CapyCanvasPocRetainedPopup"),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(windows::core::Error::from_thread().to_string());
        }
        let width = (popup.rect.w * geometry.scale).ceil().max(1.0) as u32;
        let height = (popup.rect.h * geometry.scale).ceil().max(1.0) as u32;
        let mut point = POINT {
            x: (popup.rect.x * geometry.scale).round() as i32,
            y: (popup.rect.y * geometry.scale).round() as i32,
        };
        unsafe {
            let _ = ClientToScreen(owner, &mut point);
        }
        let state = Box::new(PopupState {
            ui,
            scale: geometry.scale,
        });
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
                w!("CapyCanvasPocRetainedPopup"),
                w!(""),
                WS_POPUP,
                point.x,
                point.y,
                width as i32,
                height as i32,
                Some(owner),
                None,
                Some(module.into()),
                Some((&*state as *const PopupState).cast()),
            )
        }
        .map_err(|e| e.to_string())?;
        let result = (|| {
            let target =
                unsafe { device.CreateTargetForHwnd(hwnd, true) }.map_err(|e| e.to_string())?;
            let visual = unsafe { device.CreateVisual() }.map_err(|e| e.to_string())?;
            unsafe { target.SetRoot(&visual) }.map_err(|e| e.to_string())?;
            let surface =
                unsafe {
                    shared.instance.create_surface_unsafe(
                        wgpu::SurfaceTargetUnsafe::CompositionVisual(visual.as_raw()),
                    )
                }
                .map_err(|e| e.to_string())?;
            surface.configure(&shared.device, &config(width, height));
            let cached = Cached {
                layer: popup.layer,
                texture: texture(
                    &shared.device,
                    "cached popup content",
                    width,
                    height,
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
            };
            unsafe { device.Commit() }.map_err(|e| e.to_string())?;
            Ok::<_, String>((target, visual, surface, cached))
        })();
        match result {
            Ok((target, visual, surface, cached)) => {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                Ok(Self {
                    hwnd,
                    _state: state,
                    _target: target,
                    _visual: visual,
                    surface,
                    frame_slot: FrameSlot::default(),
                    cached,
                    chrome: texture(
                        &shared.device,
                        "popup chrome group",
                        width,
                        height,
                        wgpu::TextureFormat::Rgba8Unorm,
                    ),
                    rect: popup.rect,
                })
            }
            Err(error) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                Err(error)
            }
        }
    }
}
#[cfg(windows)]
pub fn run(
    hwnd: usize,
    options: AppOptions,
    transactions: Receiver<Transaction>,
    events: Receiver<Event>,
    ui: Sender<crate::native::UiEvent>,
    canvas: Arc<OnceLock<Canvas>>,
) -> Result<(), String> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| e.to_string())?;
    }
    let hwnd = HWND(hwnd as *mut _);
    let device: IDCompositionDevice =
        unsafe { DCompositionCreateDevice(None) }.map_err(|e| e.to_string())?;
    let target = unsafe { device.CreateTargetForHwnd(hwnd, true) }.map_err(|e| e.to_string())?;
    let root = unsafe { device.CreateVisual() }.map_err(|e| e.to_string())?;
    let lower = unsafe { device.CreateVisual() }.map_err(|e| e.to_string())?;
    let upper = unsafe { device.CreateVisual() }.map_err(|e| e.to_string())?;
    unsafe {
        root.AddVisual(&lower, false, None)
            .map_err(|e| e.to_string())?;
        root.AddVisual(&upper, true, Some(&lower))
            .map_err(|e| e.to_string())?;
        target.SetRoot(&root).map_err(|e| e.to_string())?;
    }
    let initial = crate::native::client_size(hwnd);
    let initial_scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
    canvas
        .set(Canvas::new_for_visual(
            lower,
            initial[0],
            initial[1],
            initial_scale,
            options.clone(),
        ))
        .map_err(|_| "canvas initialized twice")?;
    let canvas = canvas.get().unwrap();
    canvas.set_motion(options.load==poc_common::Load::Slow)?;
    let mut geometry = Geometry::new(
        initial[0],
        initial[1],
        initial_scale,
        metric("top_fraction"),
    );
    let mut graphics: Option<(wgpu::Surface<'static>, Paint)> = None;
    let mut layers: HashMap<LayerId, Cached> = HashMap::with_capacity(LAYER_COUNT);
    let mut pending_layers = HashMap::new();
    let mut pending_uploads = Vec::new();
    let mut dirty = HashSet::new();
    let mut state = 0;
    let mut frame8 = 0u8;
    let mut scroll = Scroll::default();
    let mut touch_contact = None;
    let mut rows = 0..0;
    let mut row_count = options.rows as u32;
    let mut drawer = -metric("drawer_width");
    let mut drawer_tween = None;
    let mut drawer_accessibility=None;
    let mut presented_drawer=None;
    let mut scroll_accessibility=None;
    let mut presented_scroll=None;
    let mut opacity = 1.0;
    let mut zen_tween = None;
    let start = Instant::now();
    let mut prior = 0.0;
    let input_start = crate::native::qpc();
    let mut input_frequency = 0;
    unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut input_frequency) }
        .map_err(|e| e.to_string())?;
    let mut cpu_at = Instant::now();
    let mut pending_popup = None;
    let mut popup_window: Option<PopupWindow> = None;
    let mut last_blur = None;
    let mut last_snapshot = None;
    let mut configured = [0; 2];
    let mut frame_slot = FrameSlot::default();
    let mut clock_reported = false;
    let mut requested_rows = None;
    let mut motion = Arc::new(Motion::default());
    let mut redraw_requested = true;
    let mut ui_initialized=false;
    let mut input_ready=false;
    let mut requested_thumbnails = Vec::new();
    let mut chrome = None;
    let cache_updates = canvas.cache_updates();
    let mut active = true;
    loop {
        if !active && popup_window.is_none() {
            let mut select = crossbeam_channel::Select::new();
            select.recv(&events);
            select.recv(&transactions);
            select.recv(&cache_updates);
            select.ready();
        }
        let wait_start = crate::native::qpc();
        let compositor_clock = clock_wait();
        if configured != [0; 2] && let Some((surface, _)) = &graphics {
            frame_slot.wait(surface)?;
        }
        if let Some(popup) = &mut popup_window {
            popup.frame_slot.wait(&popup.surface)?;
        }
        let wait_end = crate::native::qpc();
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let now = start.elapsed().as_secs_f64();
        let dt = now - prior;
        prior = now;
        let cache_changed = cache_updates.try_iter().last().is_some();
        let mut changed = redraw_requested || cache_changed;
        let mut closed = false;
        while let Ok(event) = events.try_recv() {
            match event {
                Event::Shutdown => closed = true,
                Event::Size(w, h, s) => {
                    geometry =
                        Geometry::new(w, h, s, geometry.layers.h / geometry.column.h.max(1.0));
                    changed = true;
                }
                Event::Wheel(delta) => {scroll_accessibility=None;scroll.wheel(delta, now);},
                Event::Touch(id, y, phase, receipt) => {
                    let input_seconds = (receipt - input_start) as f64 / input_frequency as f64;
                    let before = scroll.offset;
                    if phase == 0 && touch_contact.is_none() {
                        scroll_accessibility=None;
                        touch_contact = Some(id);
                        scroll.down(y, input_seconds);
                    } else if touch_contact == Some(id) {
                        if phase == 1 {
                            scroll.moved(y, input_seconds);
                        } else {
                            scroll.up(input_seconds);
                            if !scroll.active(){scroll_accessibility=Some(scroll.offset);}
                            touch_contact = None;
                        }
                    }
                    changed |= before != scroll.offset;
                }
                Event::CancelTouch(id)=>{if touch_contact==Some(id){scroll.cancel();touch_contact=None;scroll_accessibility=Some(scroll.offset);}},
                Event::Drawer(open) => {
                    drawer_accessibility=None;
                    drawer = drawer_tween.map_or(drawer, |t: Tween| t.value(now));
                    drawer_tween = Some(Tween::new(
                        drawer,
                        if open { 0.0 } else { -metric("drawer_width") },
                        now,
                        metric("drawer_ms") as f64 / 1000.0,
                        true,
                    ));
                }
                Event::Zen(show) => {
                    opacity = zen_tween.map_or(opacity, |t: Tween| t.value(now));
                    zen_tween = Some(Tween::new(
                        opacity,
                        if show { 1.0 } else { 0.0 },
                        now,
                        metric("zen_ms") as f64 / 1000.0,
                        false,
                    ));
                }
                Event::ScrollTo(y) => {
                    scroll.tween = None;
                    scroll.velocity = 0.0;
                    scroll.offset = y.clamp(0.0, scroll.maximum);
                    scroll_accessibility=Some(scroll.offset);
                    changed = true;
                }
                Event::Popup(popup) => {
                    pending_popup = Some(popup);
                    changed = true;
                }
            }
        }
        while let Ok(transaction) = transactions.try_recv() {
            ui_initialized=true;
            motion = transaction.motion;
            geometry = transaction.geometry;
            state = transaction.state;
            rows = transaction.rows;
            row_count = transaction.visible_rows;
            let old_scroll=scroll.offset;
            scroll.set_maximum(row_count as f32 * poc_common::ROW_HEIGHT - geometry.list.h);
            if scroll.offset!=old_scroll{scroll_accessibility=Some(scroll.offset);}
            for layer in transaction.layers {
                if layers.get(&layer.id).is_none_or(|cached|layer.requires_record(&cached.layer)){dirty.insert(layer.id);}
                pending_layers.insert(layer.id, layer);
            }
            pending_uploads.extend(transaction.uploads);
            changed = true;
        }
        if closed {
            break;
        }
        if canvas.is_finished(){return Err("canvas worker terminated while the compositor was active".into());}
        let Some(shared) = canvas.device() else {active = false;continue;};
        if !clock_reported {
            trace(
                canvas,
                "compositor_clock",
                serde_json::json!({"source":if compositor_clock{"DCompositionWaitForCompositorClock"}else{"DwmFlush"},"fallback":!compositor_clock}),
            );
            clock_reported = true;
        }
        if graphics.is_none() {
            unsafe{device.Commit()}.map_err(|e|e.to_string())?;
            let surface =
                unsafe {
                    shared.instance.create_surface_unsafe(
                        wgpu::SurfaceTargetUnsafe::CompositionVisual(upper.as_raw()),
                    )
                }
                .map_err(|e| e.to_string())?;
            graphics = Some((
                surface,
                Paint::new(&shared.device, &shared.queue, geometry.scale)?,
            ));
            changed = true;
            let _ = ui.send(crate::native::UiEvent::Ready);
        }
        let (surface, paint) = graphics.as_mut().unwrap();
        if paint.icon_scale != geometry.scale {
            paint.icons = icon_texture(&shared.device, &shared.queue, geometry.scale)?;
            paint.icon_scale = geometry.scale;
            dirty.extend(layers.keys().copied());
        }
        paint.upload(&shared.queue, pending_uploads.drain(..));
        for (id, layer) in pending_layers.drain() {
            let width = layer_extent(layer.rect.w * geometry.scale);
            let height = layer_extent(layer.rect.h * geometry.scale);
            if let Some(cached) = layers.get_mut(&id).filter(|cached| {
                cached.texture.width() >= width && cached.texture.height() >= height
            }) {
                cached.layer = layer;
            } else {
                layers.insert(
                    id,
                    Cached {
                        layer,
                        texture: texture(
                            &shared.device,
                            "cached retained layer",
                            width,
                            height,
                            wgpu::TextureFormat::Rgba8Unorm,
                        ),
                    },
                );
            }
        }
        let dimensions = [
            (geometry.width * geometry.scale).round().max(1.0) as u32,
            (geometry.height * geometry.scale).round().max(1.0) as u32,
        ];
        if configured != dimensions {
            surface.configure(&shared.device, &config(dimensions[0], dimensions[1]));
            chrome = None;
            configured = dimensions;
            changed = true;
            unsafe { device.Commit() }.map_err(|e| e.to_string())?;
        }
        let scroll_was_active=scroll.active();
        if !ui_initialized{active = false;continue;}
        changed |= scroll.advance(now, dt);
        if scroll_was_active&&!scroll.active(){scroll_accessibility=Some(scroll.offset);}
        if let Some(tween) = drawer_tween {
            let value = tween.value(now);
            changed |= value != drawer;
            drawer = value;
            if !tween.active(now) {
                drawer_tween = None;
                drawer_accessibility=Some(drawer);
            }
        }
        if let Some(tween) = zen_tween {
            let value = tween.value(now);
            changed |= value != opacity;
            opacity = value;
            if !tween.active(now) {
                zen_tween = None;
            }
        }
        motion.scroll.store(
            scroll.offset.to_bits(),
            std::sync::atomic::Ordering::Release,
        );
        motion
            .drawer
            .store(drawer.to_bits(), std::sync::atomic::Ordering::Release);
        if needs_rows(&rows, scroll.offset, geometry.list.h, row_count) {
            let needed = recorded_rows(scroll.offset, geometry.list.h, row_count);
            if requested_rows.as_ref() != Some(&needed)
                && ui
                    .try_send(crate::native::UiEvent::NeedRows(scroll.offset))
                    .is_ok()
            {
                requested_rows = Some(needed);
            }
        }
        if let Some(list) = layers.get(&LayerId::List) {
            let visible: Vec<_> = list
                .layer
                .display
                .iter()
                .filter_map(|primitive| {
                    let Prim::Image { rect, atlas, .. } = primitive else {
                        return None;
                    };
                    (*atlas > 0
                        && rect.y - scroll.offset < geometry.list.y + geometry.list.h
                        && rect.y + rect.h - scroll.offset > geometry.list.y)
                        .then(|| *atlas - 1)
                })
                .take(poc_common::THUMBNAIL_CAPACITY as usize)
                .collect();
            if visible != requested_thumbnails {
                canvas.set_thumbnail_rows(&visible)?;
                requested_thumbnails = visible;
                changed = true;
            }
        }
        let snapshot = shared.cache.snapshot().or_else(|| last_snapshot.clone());
        if snapshot.is_none() {
            redraw_requested = changed || !dirty.is_empty();
            active = false;
            continue;
        }
        changed |= !dirty.is_empty();
        let generation = snapshot.as_ref().map(|snapshot| {
            snapshot
                .generation
                .load(std::sync::atomic::Ordering::Acquire)
        });
        changed |= generation != last_blur
            || snapshot.as_ref().is_some_and(|snapshot| {
                last_snapshot
                    .as_ref()
                    .is_none_or(|last| !Arc::ptr_eq(snapshot, last))
            });
        if cpu_at.elapsed() >= Duration::from_secs(1) {
            trace(
                canvas,
                "thread_cpu",
                serde_json::json!({"thread":"compositor","cpu_ms":crate::native::cpu_ms()}),
            );
            cpu_at = Instant::now();
        }
        frame_slot.wait(surface)?;
        let mut encoder = shared
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("retained frame"),
            });
        if let Some(popup) = pending_popup.take() {
            if let Some(popup) = &popup { paint.upload(&shared.queue, popup.uploads.clone()); }
            popup_window = popup
                .map(|popup| PopupWindow::new(hwnd, popup, geometry, shared, &device, ui.clone()))
                .transpose()?;
            if let Some(popup) = &mut popup_window {
                popup.frame_slot.wait(&popup.surface)?;
                paint.record(&shared.device, &mut encoder, &popup.cached, geometry);
            }
        }
        if !changed && !scroll.active() && drawer_tween.is_none() && zen_tween.is_none() {
            if scroll_accessibility==presented_scroll&&let Some(offset)=scroll_accessibility.take(){let _=ui.try_send(crate::native::UiEvent::ScrollSettled(offset));}
            if drawer_accessibility==presented_drawer&& let Some(x)=drawer_accessibility.take(){let _=ui.try_send(crate::native::UiEvent::DrawerSettled(x));}
            active = false;
            continue;
        }
        let dirty_start = crate::native::qpc();
        let dirty_count = dirty.len();
        for id in dirty.drain() {
            if let Some(layer) = layers.get(&id) {
                paint.record(&shared.device, &mut encoder, layer, geometry);
            }
        }
        let composite_start = crate::native::qpc();
        let frame = acquire(surface)?;
        let view = frame.texture.create_view(&Default::default());
        let fade_group = zen_tween.is_some() || opacity < 1.0;
        if fade_group && chrome.as_ref().is_none_or(|texture: &wgpu::Texture| texture.width() != dimensions[0] || texture.height() != dimensions[1]) {
            chrome = Some(texture(&shared.device, "retained chrome opacity group", dimensions[0], dimensions[1], wgpu::TextureFormat::Rgba8Unorm));
        }
        let chrome_view = if fade_group { chrome.as_ref().unwrap().create_view(&Default::default()) } else { view.clone() };
        paint.clear(&chrome_view);
        let screen = [dimensions[0] as f32, dimensions[1] as f32, 0.0, 0.0];
        let frame_info = Frame {
            size: screen,
            origin: [0.0, 0.0, screen[0], screen[1]],
            clip: [0.0, 0.0, screen[0], screen[1]],
            mask: [0.0, 0.0, screen[0], screen[1]],
        };
        paint.compose_layers(&shared.device,&mut encoder,SurfacePaint{view:&chrome_view,frame:frame_info},&layers,Composition{geometry,drawer,scroll:scroll.offset},snapshot.as_deref());
        if fade_group {
            paint.clear(&view);
            paint.composite_group(&shared.device, &mut encoder, SurfacePaint { view: &view, frame: frame_info }, &chrome_view, opacity);
        }
        if let Some(cached) = layers.get(&LayerId::Marker) {
            paint.compose(
                &shared.device,
                &mut encoder,
                SurfacePaint {
                    view: &view,
                    frame: frame_info,
                },
                cached,
                Placement {
                    rect: cached.layer.rect,
                    scale: geometry.scale,
                    opacity: 1.0,
                },
                snapshot.as_deref(),
            );
        }
        frame8 = frame8.wrapping_add(1);
        let marker = poc_common::ui_marker(state, frame8);
        let mut guard = Instance::new(Rect::new(3.0, 43.0, 98.0, 6.0));
        guard.color = poc_common::MARKER_GUARD.map(|c| c as f32 / 255.0);
        let mut markers = vec![guard];
        for bit in 0..24 {
            let c = if poc_common::marker_bit(marker, bit) {
                1.0
            } else {
                0.0
            };
            let mut item = Instance::new(Rect::new(4.0 + bit as f32 * 4.0, 44.0, 4.0, 4.0));
            item.color = [c, c, c, 1.0];
            markers.push(item);
        }
        paint.draw(
            &shared.device,
            &mut encoder,
            SurfacePaint {
                view: &view,
                frame: frame_info,
            },
            &markers,
            &paint.white.create_view(&Default::default()),
            0,
        );
        let popup_frame = if let Some(popup) = &popup_window {
            let frame = acquire(&popup.surface)?;
            let view = frame.texture.create_view(&Default::default());
            let popup_chrome = if fade_group { popup.chrome.create_view(&Default::default()) } else { view.clone() };
            paint.clear(&popup_chrome);
            let mut info = frame_info;
            info.size = [
                popup.cached.texture.width() as f32,
                popup.cached.texture.height() as f32,
                0.0,
                0.0,
            ];
            info.origin[0] = popup.rect.x * geometry.scale;
            info.origin[1] = popup.rect.y * geometry.scale;
            info.clip = geometry.physical(popup.rect).array();
            info.mask = info.clip;
            paint.compose(
                &shared.device,
                &mut encoder,
                SurfacePaint {
                    view: &popup_chrome,
                    frame: info,
                },
                &popup.cached,
                Placement {
                    rect: Rect::new(0.0, 0.0, popup.rect.w, popup.rect.h),
                    scale: geometry.scale,
                    opacity: 1.0,
                },
                snapshot.as_deref(),
            );
            if fade_group {
                paint.clear(&view);
                paint.composite_group(&shared.device, &mut encoder, SurfacePaint { view: &view, frame: info }, &popup_chrome, opacity);
            }
            Some(frame)
        } else {
            None
        };
        paint.flush(&shared.device, &shared.queue, &mut encoder);
        let submit_qpc = crate::native::qpc();
        shared.queue.submit_for_present([encoder.finish()]);
        frame_slot.present(&shared.queue, frame);
        if let Some(frame) = popup_frame {
            popup_window.as_mut().unwrap().frame_slot.present(&shared.queue, frame);
        }
        let present_qpc = crate::native::qpc();
        unsafe { device.Commit() }.map_err(|e| e.to_string())?;
        if ui_initialized&&!input_ready{
            trace(canvas,"startup",serde_json::json!({"milestone":"input_ready","first_ui_present_qpc":present_qpc}));
            if let Some(name)=&options.ready_event{poc_common::ready::signal(name)?;}
            input_ready=true;
        }
        presented_scroll=Some(scroll.offset);
        if scroll_accessibility==presented_scroll&&let Some(offset)=scroll_accessibility.take(){let _=ui.try_send(crate::native::UiEvent::ScrollSettled(offset));}
        presented_drawer=Some(drawer);
        if drawer_accessibility==presented_drawer&& let Some(x)=drawer_accessibility.take(){let _=ui.try_send(crate::native::UiEvent::DrawerSettled(x));}
        last_blur = generation;
        last_snapshot = snapshot;
        redraw_requested = false;
        active = scroll.active() || drawer_tween.is_some() || zen_tween.is_some();
        trace(
            canvas,
            "ui_present",
            serde_json::json!({"wait_start_qpc":wait_start,"wait_end_qpc":wait_end,"animate_end_qpc":dirty_start,"dirty_end_qpc":composite_start,"submit_qpc":submit_qpc,"present_qpc":present_qpc,"dirty_layers":dirty_count,"scroll_offset":scroll.offset,"drawer_x":drawer,"opacity":opacity,"state":state,"ui_frame":frame8}),
        );
    }
    drop(popup_window);
    trace(
        canvas,
        "thread_cpu",
        serde_json::json!({"thread":"compositor","cpu_ms":crate::native::cpu_ms()}),
    );
    canvas.shutdown()?;
    while !canvas.is_finished() {
        clock_wait();
    }
    unsafe { PostMessageW(Some(hwnd), WM_APP + 42, WPARAM(0), LPARAM(0)) }
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gpu() -> (wgpu::Device, wgpu::Queue) {
        static INSTANCE:std::sync::OnceLock<wgpu::Instance>=std::sync::OnceLock::new();
        let instance=INSTANCE.get_or_init(||{let mut descriptor=wgpu::InstanceDescriptor::new_without_display_handle();descriptor.backends=wgpu::Backends::PRIMARY;descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);wgpu::Instance::new(descriptor)}).clone();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        pollster::block_on(adapter.request_device(&Default::default())).unwrap()
    }
    fn pixels(
        paint: &Paint,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut encoder: wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) -> Vec<u8> {
        paint.flush(device, queue, &mut encoder);
        let stride = (source.width() * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (stride * source.height()) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            source.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            source.size(),
        );
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer.slice(..).get_mapped_range().unwrap();
        mapped
            .chunks(stride as usize)
            .flat_map(|row| row[..source.width() as usize * 4].iter().copied())
            .collect()
    }
    #[test]
    fn title_width_probe_stays_two_physical_pixels_when_the_native_border_covers_the_client_edge() {
        let (device,queue)=gpu();
        for (width,scale) in [(2000,1.25),(2400,1.5),(2100,1.5)] {
            let geometry=Geometry::new(width,1500,scale,0.6);
            let paint=Paint::new(&device,&queue,scale).unwrap();
            let rect=Rect::new(0.0,0.0,geometry.width,metric("title_height"));
            let height=(rect.h*scale).round() as u32;
            let cached=Cached{layer:Layer{id:LayerId::Title,rect,clip:rect,origin:[0.0;2],display:Arc::new(vec![Prim::Probe{rect:geometry.width_probe(),color:[0.0,0.0,1.0,1.0]}])},texture:texture(&device,"title cache",layer_extent(width as f32),layer_extent(height as f32),wgpu::TextureFormat::Rgba8Unorm)};
            let output=texture(&device,"title client",width,height,wgpu::TextureFormat::Rgba8Unorm);
            let view=output.create_view(&Default::default());
            let frame=Frame{size:[width as f32,height as f32,0.0,0.0],origin:[0.0,0.0,width as f32,height as f32],clip:[0.0,0.0,width as f32,height as f32],mask:[0.0,0.0,width as f32,height as f32]};
            let mut encoder=device.create_command_encoder(&Default::default());
            paint.record(&device,&mut encoder,&cached,geometry);clear(&mut encoder,&view);
            paint.compose(&device,&mut encoder,SurfacePaint{view:&view,frame},&cached,Placement{rect,scale,opacity:1.0},None);
            let mut border=Instance::new(Rect::new(width as f32-2.0,0.0,2.0,height as f32));border.color=[58.0/255.0,58.0/255.0,58.0/255.0,1.0];
            paint.draw(&device,&mut encoder,SurfacePaint{view:&view,frame},&[border],&paint.white.create_view(&Default::default()),0);
            let bytes=pixels(&paint,&device,&queue,encoder,&output);
            let y=(20.0*scale).round() as usize;
            let blue:Vec<_>=(0..width as usize).filter(|x|bytes[(y*width as usize+x)*4..(y*width as usize+x)*4+4]==[0,0,255,255]).collect();
            assert_eq!(blue,[(width-4) as usize,(width-3) as usize]);
        }
    }
    #[test]
    fn five_pipelines_preserve_live_glass_sparse_thumbnail_ids_and_exact_outset_probe() {
        let (device, queue) = gpu();
        let paint = Paint::new(&device, &queue, 1.5).unwrap();
        let geometry = Geometry::new(192, 192, 1.5, 0.6);
        let source = texture(
            &device,
            "blur source",
            192,
            192,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let mut cache = poc_canvas::cache::Cache::new(&device, 192, 192);
        let mut encoder = device.create_command_encoder(&Default::default());
        clear(&mut encoder, &source.create_view(&Default::default()));
        let generation = cache.encode_blur(
            &device,
            &queue,
            &mut encoder,
            &source.create_view(&Default::default()),
        );
        cache.encode_thumbnails(&device, &queue, &mut encoder, &[70], 0.0);
        queue.submit([encoder.finish()]);
        cache.publish_blur(generation);
        let snapshot = poc_canvas::cache::CacheSnapshot {
            blur_views: cache.blur_views.clone(),
            generation: cache.generation.clone(),
            thumbnail_view: cache.thumbnail_view.clone(),
            thumbnail_rows: Arc::from([70]),
            dimensions: [192, 192],
        };
        paint.upload(
            &queue,
            [GlyphUpload {
                origin: [0, 0],
                size: [4, 4],
                pixels: vec![255; 16],
            }],
        );
        let rect = Rect::new(8.0, 8.0, 64.0, 48.0);
        let display = vec![
            Prim::Glass { rect, radius: 12.0 },
            Prim::Glyph {
                rect: Rect::new(16.0, 16.0, 8.0, 8.0),
                uv: [
                    0.0,
                    0.0,
                    4.0 / crate::text::ATLAS_SIZE as f32,
                    4.0 / crate::text::ATLAS_SIZE as f32,
                ],
                color: [0.0, 0.0, 1.0, 1.0],
            },
            Prim::Image {
                rect: Rect::new(16.0, 32.0, 8.0, 8.0),
                uv: [0.0, 0.0, 1.0, 1.0],
                atlas: 71,
            },
            Prim::Image {
                rect: Rect::new(48.0, 32.0, 8.0, 8.0),
                uv: [0.0, 0.0, 1.0, 1.0],
                atlas: 1,
            },
            Prim::Probe {
                rect: Rect::new(72.0, 24.0, 2.0 / 1.5, 8.0),
                color: [1.0, 1.0, 0.0, 1.0],
            },
        ];
        let cached = Cached {
            layer: Layer {
                id: LayerId::Properties,
                rect,
                clip: Rect::new(0.0, 0.0, 128.0, 128.0),
                display: Arc::new(display),
                origin: [0.0; 2],
            },
            texture: texture(
                &device,
                "cached glyphs",
                96,
                72,
                wgpu::TextureFormat::Rgba8Unorm,
            ),
        };
        let output = texture(
            &device,
            "retained output",
            192,
            192,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let frame = Frame {
            size: [192.0, 192.0, 0.0, 0.0],
            origin: [0.0, 0.0, 192.0, 192.0],
            clip: [0.0, 0.0, 192.0, 192.0],
            mask: [0.0, 0.0, 192.0, 192.0],
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        paint.record(&device, &mut encoder, &cached, geometry);
        clear(&mut encoder, &output.create_view(&Default::default()));
        paint.compose(
            &device,
            &mut encoder,
            SurfacePaint {
                view: &output.create_view(&Default::default()),
                frame,
            },
            &cached,
            Placement {
                rect,
                scale: 1.5,
                opacity: 1.0,
            },
            Some(&snapshot),
        );
        let first = pixels(&paint,&device, &queue, encoder, &output);
        let pixel = |bytes: &[u8], x: usize, y: usize| {
            <[u8; 4]>::try_from(&bytes[(y * 192 + x) * 4..(y * 192 + x + 1) * 4]).unwrap()
        };
        assert_eq!(pixel(&first, 30, 30), [0, 0, 255, 255]);
        assert_eq!(pixel(&first, 108, 42), [255, 255, 0, 255]);
        assert_eq!(pixel(&first, 110, 42), [0, 0, 0, 0]);
        let mut shifted = device.create_command_encoder(&Default::default());
        clear(&mut shifted, &output.create_view(&Default::default()));
        paint.compose(
            &device,
            &mut shifted,
            SurfacePaint {
                view: &output.create_view(&Default::default()),
                frame,
            },
            &cached,
            Placement {
                rect: rect.translated(0.25, 0.0),
                scale: 1.5,
                opacity: 1.0,
            },
            Some(&snapshot),
        );
        let shifted = pixels(&paint,&device, &queue, shifted, &output);
        assert_eq!(
            (0..192)
                .filter(|x| pixel(&shifted, *x, 42) == [255, 255, 0, 255])
                .collect::<Vec<_>>(),
            [108, 109]
        );
        assert_eq!(
            (0..192)
                .filter(|y| pixel(&first, 110, *y) == [0, 255, 255, 255])
                .collect::<Vec<_>>(),
            [84, 85]
        );
        let mut clipped=device.create_command_encoder(&Default::default());clear(&mut clipped,&output.create_view(&Default::default()));let mut clipped_frame=frame;clipped_frame.clip=geometry.physical(rect).array();paint.compose(&device,&mut clipped,SurfacePaint{view:&output.create_view(&Default::default()),frame:clipped_frame},&cached,Placement{rect,scale:1.5,opacity:1.0},Some(&snapshot));let clipped=pixels(&paint,&device,&queue,clipped,&output);assert_eq!((0..192).filter(|y|pixel(&clipped,110,*y)==[0,255,255,255]).collect::<Vec<_>>(),[84,85]);assert_eq!(pixel(&clipped,108,42),[0,0,0,0]);
        let background = pixel(&first, 78, 54);
        assert_eq!(background[3], 255);
        assert!(background[0] < 40 && background[1] < 40);
        let thumbnail = pixel(&first, 30, 54);
        assert_ne!(thumbnail, background);
        assert_eq!(thumbnail[3], 255);
        let mut encoder = device.create_command_encoder(&Default::default());
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &source.create_view(&Default::default()),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));
        let generation = cache.encode_blur(
            &device,
            &queue,
            &mut encoder,
            &source.create_view(&Default::default()),
        );
        queue.submit([encoder.finish()]);
        cache.publish_blur(generation);
        let staging = texture(
            &device,
            "full opacity chrome group",
            192,
            192,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let staging_view = staging.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        clear(&mut encoder, &staging_view);
        clear(&mut encoder, &output.create_view(&Default::default()));
        paint.compose(
            &device,
            &mut encoder,
            SurfacePaint {
                view: &staging_view,
                frame,
            },
            &cached,
            Placement {
                rect,
                scale: 1.5,
                opacity: 1.0,
            },
            Some(&snapshot),
        );
        paint.composite_group(
            &device,
            &mut encoder,
            SurfacePaint {
                view: &output.create_view(&Default::default()),
                frame,
            },
            &staging_view,
            0.5,
        );
        let second = pixels(&paint,&device, &queue, encoder, &output);
        let glyph = pixel(&second, 30, 30);
        assert_eq!(glyph[0], 0);
        assert_eq!(glyph[1], 0);
        assert!((glyph[2] as i32 - 128).abs() <= 1);
        assert!((glyph[3] as i32 - 128).abs() <= 1);
        let changed = pixel(&second, 78, 54);
        assert!(changed[0] > background[0]);
        assert!((changed[3] as i32 - 128).abs() <= 1);
        for y in [84, 85] {
            let p = pixel(&second, 110, y);
            assert_eq!(p[0], 0);
            for channel in [1, 2, 3] {
                assert!((p[channel] as i32 - 128).abs() <= 1);
            }
        }
    }
    #[test]
    fn list_content_cannot_cover_the_parent_group_bottom_probes(){
        let(device,queue)=gpu();let paint=Paint::new(&device,&queue,1.5).unwrap();
        paint.upload(&queue,[GlyphUpload{origin:[0,0],size:[4,4],pixels:vec![255;16]}]);
        let mut geometry=Geometry::new(576,192,1.5,0.6);geometry.layers=Rect::new(8.0,8.0,304.0,96.0);geometry.list=Rect::new(8.0,44.0,304.0,60.0);
        let group=geometry.layers;let px=1.0/geometry.scale;let mut layers=HashMap::new();
        layers.insert(LayerId::Layers,Cached{layer:Layer{id:LayerId::Layers,rect:group,clip:Rect::new(0.0,0.0,384.0,128.0),origin:[0.0;2],display:Arc::new(vec![
            Prim::Rect{rect:group,color:[0.1,0.1,0.1,1.0],radius:12.0,border:0.0,border_color:[0.0;4]},
            Prim::Probe{rect:Rect::new(group.x,group.y+group.h-2.0*px,group.w/3.0,2.0*px),color:[1.0,0.0,1.0,1.0]},
            Prim::Probe{rect:Rect::new(group.x+group.w*2.0/3.0,group.y+group.h-px,group.w/3.0,px),color:[1.0,128.0/255.0,0.0,1.0]},
        ])},texture:texture(&device,"parent group cache",456,144,wgpu::TextureFormat::Rgba8Unorm)});
        layers.insert(LayerId::List,Cached{layer:Layer{id:LayerId::List,rect:geometry.list,clip:geometry.list,origin:[0.0;2],display:Arc::new(vec![Prim::Glyph{rect:Rect::new(group.x+48.0,group.y+group.h-4.0,12.0,12.0),uv:[0.0,0.0,4.0/crate::text::ATLAS_SIZE as f32,4.0/crate::text::ATLAS_SIZE as f32],color:[1.0;4]},Prim::Glyph{rect:Rect::new(group.x+248.0,group.y+group.h-4.0,12.0,12.0),uv:[0.0,0.0,4.0/crate::text::ATLAS_SIZE as f32,4.0/crate::text::ATLAS_SIZE as f32],color:[1.0;4]}])},texture:texture(&device,"list content cache",456,90,wgpu::TextureFormat::Rgba8Unorm)});
        let output=texture(&device,"parent probe output",576,192,wgpu::TextureFormat::Rgba8Unorm);let view=output.create_view(&Default::default());let frame=Frame{size:[576.0,192.0,0.0,0.0],origin:[0.0,0.0,576.0,192.0],clip:[0.0,0.0,576.0,192.0],mask:[0.0,0.0,576.0,192.0]};let mut encoder=device.create_command_encoder(&Default::default());
        for cached in layers.values(){paint.record(&device,&mut encoder,cached,geometry);}clear(&mut encoder,&view);
        paint.compose_layers(&device,&mut encoder,SurfacePaint{view:&view,frame},&layers,Composition{geometry,drawer:0.0,scroll:0.0},None);
        let bytes=pixels(&paint,&device,&queue,encoder,&output);let pixel=|x:usize,y:usize|&bytes[(y*576+x)*4..(y*576+x+1)*4];
        for y in [154,155]{assert_eq!(pixel(88,y),&[255,0,255,255]);}assert_eq!(pixel(388,155),&[255,128,0,255]);
    }
    #[test]
    fn one_layer_pass_orders_rect_image_glyph_ranges_and_samples_only_its_live_extent() {
        let (device,queue)=gpu();let paint=Paint::new(&device,&queue,1.0).unwrap();
        paint.upload(&queue,[GlyphUpload{origin:[0,0],size:[4,4],pixels:vec![255;16]}]);
        let rect=Rect::new(0.0,0.0,64.0,32.0);
        let cached=Cached{layer:Layer{id:LayerId::Properties,rect,clip:rect,origin:[0.0;2],display:Arc::new(vec![
            Prim::Glyph{rect:Rect::new(8.0,8.0,8.0,8.0),uv:[0.0,0.0,4.0/crate::text::ATLAS_SIZE as f32,4.0/crate::text::ATLAS_SIZE as f32],color:[0.0,0.0,1.0,1.0]},
            Prim::Image{rect:Rect::new(24.0,8.0,16.0,16.0),uv:[0.0,0.0,1.0,1.0],atlas:0},
            Prim::Rect{rect,color:[1.0,0.0,0.0,1.0],radius:0.0,border:0.0,border_color:[0.0;4]},
        ])},texture:texture(&device,"slack layer",128,128,wgpu::TextureFormat::Rgba8Unorm)};
        let output=texture(&device,"live extent",64,64,wgpu::TextureFormat::Rgba8Unorm);let view=output.create_view(&Default::default());
        let frame=Frame{size:[64.0,64.0,0.0,0.0],origin:[0.0,0.0,64.0,64.0],clip:[0.0,0.0,64.0,64.0],mask:[0.0,0.0,64.0,64.0]};
        let mut encoder=device.create_command_encoder(&Default::default());
        paint.record(&device,&mut encoder,&cached,Geometry::new(64,64,1.0,0.6));
        {
            let plan=paint.frame.borrow();assert_eq!(plan.targets.len(),1);assert!(plan.targets[0].clear);
            assert_eq!(plan.targets[0].batches.iter().map(|batch|batch.pipeline).collect::<Vec<_>>(),[0,2,1]);
            assert_eq!(plan.targets[0].batches.iter().map(|batch|batch.instances.clone()).collect::<Vec<_>>(),[0..1,1..2,2..3]);
        }
        clear(&mut encoder,&view);paint.compose(&device,&mut encoder,SurfacePaint{view:&view,frame},&cached,Placement{rect,scale:1.0,opacity:1.0},None);
        let bytes=pixels(&paint,&device,&queue,encoder,&output);let pixel=|x:usize,y:usize|&bytes[(y*64+x)*4..(y*64+x+1)*4];
        assert_eq!(pixel(10,10),&[0,0,255,255]);assert_eq!(pixel(50,25),&[255,0,0,255]);assert_eq!(pixel(50,34),&[0,0,0,0]);
        let buffer=paint.buffer.borrow().as_ref().unwrap().buffer.clone();
        let mut encoder=device.create_command_encoder(&Default::default());clear(&mut encoder,&view);
        paint.compose(&device,&mut encoder,SurfacePaint{view:&view,frame},&cached,Placement{rect,scale:1.0,opacity:1.0},None);
        let bytes=pixels(&paint,&device,&queue,encoder,&output);assert_eq!(&bytes[(25*64+50)*4..(25*64+51)*4],&[255,0,0,255]);
        assert_eq!(paint.buffer.borrow().as_ref().unwrap().buffer,buffer);
        assert!(paint.sources.borrow().keys().all(|texture| *texture==paint.white || *texture==paint.glyphs || *texture==paint.icons || *texture==cached.texture));
        let retired=cached.texture.clone();let cached=Cached{layer:cached.layer,texture:texture(&device,"replacement layer",128,128,wgpu::TextureFormat::Rgba8Unorm)};
        let mut encoder=device.create_command_encoder(&Default::default());paint.record(&device,&mut encoder,&cached,Geometry::new(64,64,1.0,0.6));clear(&mut encoder,&view);
        paint.compose(&device,&mut encoder,SurfacePaint{view:&view,frame},&cached,Placement{rect,scale:1.0,opacity:1.0},None);
        let bytes=pixels(&paint,&device,&queue,encoder,&output);assert_eq!(&bytes[(10*64+10)*4..(10*64+11)*4],&[0,0,255,255]);
        assert!(!paint.sources.borrow().contains_key(&retired));assert_eq!(paint.buffer.borrow().as_ref().unwrap().buffer,buffer);
        assert_eq!(layer_extent(63.2),64);assert_eq!(layer_extent(64.1),128);
    }
    #[test]
    fn r8_glyph_pipeline_applies_gamma_and_contrast_with_premultiplied_alpha() {
        let (device, queue) = gpu();
        let paint = Paint::new(&device, &queue, 1.0).unwrap();
        paint.upload(
            &queue,
            [GlyphUpload {
                origin: [0, 0],
                size: [4, 4],
                pixels: vec![128; 16],
            }],
        );
        let output = texture(
            &device,
            "glyph correction output",
            8,
            8,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        let view = output.create_view(&Default::default());
        clear(&mut encoder, &view);
        let frame = Frame {
            size: [8.0, 8.0, 0.0, 0.0],
            origin: [0.0, 0.0, 8.0, 8.0],
            clip: [0.0, 0.0, 8.0, 8.0],
            mask: [0.0, 0.0, 8.0, 8.0],
        };
        let mut item = Instance::new(Rect::new(0.0, 0.0, 8.0, 8.0));
        item.uv = [
            0.0,
            0.0,
            4.0 / crate::text::ATLAS_SIZE as f32,
            4.0 / crate::text::ATLAS_SIZE as f32,
        ];
        paint.draw(
            &device,
            &mut encoder,
            SurfacePaint { view: &view, frame },
            &[item],
            &paint.glyphs.create_view(&Default::default()),
            1,
        );
        let bytes = pixels(&paint,&device, &queue, encoder, &output);
        let center = &bytes[(4 * 8 + 4) * 4..(4 * 8 + 5) * 4];
        assert!(center[0] > 170 && center[0] < 185);
        assert!(center.iter().all(|channel| *channel == center[0]));
    }
}

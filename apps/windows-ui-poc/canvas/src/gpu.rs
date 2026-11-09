use poc_common::PenSample;
use windows::{core::Interface,Win32::{Foundation::{HANDLE, WAIT_OBJECT_0}, System::Threading::WaitForSingleObject}};

#[derive(Default)]
struct FrameSlot {reserved:bool,chain:usize}
impl FrameSlot {
    fn acquire(&mut self,chain:usize,wait:impl FnOnce()->Result<(),String>)->Result<(),String>{
        if self.chain!=chain{self.chain=chain;self.reserved=false;}
        if !self.reserved {wait()?;self.reserved=true;}Ok(())
    }
    fn release(&mut self){self.reserved=false;}
}
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub info: wgpu::AdapterInfo,
    pub scale: f32,
    frame_slot:FrameSlot,
    pub document: wgpu::Texture,
    pub viewport: wgpu::Texture,
    pub viewport_view: wgpu::TextureView,
    params: wgpu::Buffer,
    dabs: wgpu::Buffer,
    document_group: wgpu::BindGroup,
    viewport_group: wgpu::BindGroup,
    viewport_pipeline: wgpu::RenderPipeline,
    dab_pipeline: wgpu::RenderPipeline,
    upload: wgpu::util::StagingBelt,
    pub timestamps: Option<crate::cache::GpuTimestamps>,
    pub in_flight: Option<wgpu::SubmissionIndex>,
}

pub fn qpc() -> i64 {
    let mut value = 0;
    unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut value).expect("QPC"); }
    value
}
pub fn frequency() -> i64 {
    let mut value = 0;
    unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut value).expect("QPC frequency"); }
    value
}
pub fn texture(device: &wgpu::Device, label: &str, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 }, mip_level_count: 1,
        sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn shader_source()->String {
    let [r,g,b,_]=poc_common::MARKER_GUARD;
    include_str!("canvas.wgsl").replace("MARKER_GUARD_RGB",&format!("vec3<f32>({r}.0,{g}.0,{b}.0)/255.0"))
}
fn pipeline(device: &wgpu::Device, layout: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule, vertex: &str, fragment: &str) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fragment), layout: Some(layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some(vertex), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(fragment), compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
        multiview_mask: None, cache: None,
    })
}
fn draw(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup, range: [u32;2], clear: bool, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("canvas"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: if clear { wgpu::LoadOp::Clear(wgpu::Color::BLACK) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store } })],
        timestamp_writes, ..Default::default()
    });
    pass.set_pipeline(pipeline); pass.set_bind_group(0, group, &[]); pass.draw(0..range[0], 0..range[1]);
}
struct DabPass<'a> {
    document: &'a wgpu::Texture,
    buffer: &'a wgpu::Buffer,
    group: &'a wgpu::BindGroup,
    pipeline: &'a wgpu::RenderPipeline,
    size: [u32; 2],
}
impl DabPass<'_> {
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, upload: &mut wgpu::util::StagingBelt, samples: &[PenSample]) {
        for chunk in samples.chunks(16384) {
            let mut bytes = Vec::with_capacity(chunk.len() * 16);
            for s in chunk {
                for value in [s.x / self.size[0] as f32, s.y / self.size[1] as f32, s.pressure, 0.0] { bytes.extend_from_slice(&value.to_ne_bytes()); }
            }
            if !bytes.is_empty() {
                upload.write_buffer(encoder,self.buffer,0,wgpu::BufferSize::new(bytes.len() as u64).unwrap()).copy_from_slice(&bytes);
                draw(encoder, &self.document.create_view(&Default::default()), self.pipeline, self.group, [6, chunk.len() as u32], false, None);
            }
        }
    }
}
impl Gpu {
    pub fn instance() -> wgpu::Instance {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        desc.flags.remove(wgpu::InstanceFlags::DEBUG);
        desc.backend_options.dx12.latency_waitable_object = wgpu::Dx12UseFrameLatencyWaitableObject::DontWait;
        wgpu::Instance::new(desc)
    }
    pub fn new(instance: wgpu::Instance, surface: wgpu::Surface<'static>, width: u32, height: u32, scale: f32) -> Result<Self, String> {
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        adapters.sort_by_key(|a| a.get_info().device_type == wgpu::DeviceType::Cpu);
        let adapter = adapters.into_iter().find(|a| a.is_surface_supported(&surface)).ok_or("no D3D12 adapter supports composition surface")?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("POC shared GPU"), required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY, ..Default::default() })).map_err(|e| e.to_string())?;
        let mut config = surface.get_default_config(&adapter, width.max(1), height.max(1)).ok_or("surface has no configuration")?;
        let caps = surface.get_capabilities(&adapter);
        if !caps.formats.contains(&wgpu::TextureFormat::Rgba8Unorm) { return Err("composition surface lacks Rgba8Unorm".into()); }
        if !caps.usages.contains(wgpu::TextureUsages::COPY_DST) { return Err("composition surface lacks GPU copy destination usage".into()); }
        config.format = wgpu::TextureFormat::Rgba8Unorm; config.view_formats.clear();
        config.present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Immediate) { wgpu::PresentMode::Immediate } else { wgpu::PresentMode::Fifo };
        config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        config.desired_maximum_frame_latency = 1;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST;
        surface.configure(&device, &config);
        let document = texture(&device, "4096 document", 4096, 4096);
        let dummy = texture(&device, "unused document input", 1, 1);
        let viewport = texture(&device, "viewport", width.max(1), height.max(1));
        let viewport_view=viewport.create_view(&Default::default());
        let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("canvas params"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let dabs = device.create_buffer(&wgpu::BufferDescriptor { label: Some("pen dabs"), size: 16384 * 16, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("canvas bindings"), entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("canvas"), source: wgpu::ShaderSource::Wgsl(shader_source().into()) });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let group = |input: &wgpu::Texture| device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&input.create_view(&Default::default())) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            wgpu::BindGroupEntry { binding: 3, resource: dabs.as_entire_binding() },
        ] });
        let document_group = group(&dummy); let viewport_group = group(&document);
        let initialize_pipeline = pipeline(&device, &pipeline_layout, &shader, "fullscreen", "initialize");
        let viewport_pipeline = pipeline(&device, &pipeline_layout, &shader, "fullscreen", "viewport");
        let dab_pipeline = pipeline(&device, &pipeline_layout, &shader, "dab_vertex", "dab_fragment");
        let mut encoder = device.create_command_encoder(&Default::default());
        draw(&mut encoder, &document.create_view(&Default::default()), &initialize_pipeline, &document_group, [3, 1], true, None);
        queue.submit([encoder.finish()]);
        let upload = wgpu::util::StagingBelt::new(device.clone(), 1 << 20);
        let timestamps = crate::cache::GpuTimestamps::new(&device);
        Ok(Self { device, queue, surface, config, info, scale, frame_slot:FrameSlot::default(),document, viewport, viewport_view, params, dabs, document_group, viewport_group, viewport_pipeline, dab_pipeline, upload, timestamps, in_flight: None })
    }
    pub fn resize(&mut self, width: u32, height: u32, scale: f32) {
        self.config.width = width.max(1); self.config.height = height.max(1); self.scale = scale;
        self.surface.configure(&self.device, &self.config);
        self.viewport = texture(&self.device, "viewport", width.max(1), height.max(1));
        self.viewport_view=self.viewport.create_view(&Default::default());
    }
    pub fn set_panel_transform(&self)->Result<(),String> {
        let native=unsafe{self.surface.as_hal::<wgpu::hal::api::Dx12>()}.ok_or("panel surface has no D3D12 backend")?;
        let chain=native.swap_chain().ok_or("panel swap chain is not configured")?;
        let extent=unsafe{chain.GetDesc1()}.map_err(|error|error.to_string())?;
        let transform=windows::Win32::Graphics::Dxgi::DXGI_MATRIX_3X2_F {_11:self.config.width as f32/extent.Width as f32/self.scale,_22:self.config.height as f32/extent.Height as f32/self.scale,..Default::default()};
        unsafe{chain.SetSourceSize(self.config.width,self.config.height).and_then(|()|chain.SetMatrixTransform(&transform))}.map_err(|error|error.to_string())
    }
    pub fn wait(&mut self) -> Result<(), String> {
        if let Some(submission) = self.in_flight.take() {
            self.device.poll(wgpu::PollType::Wait { submission_index: Some(submission), timeout: Some(std::time::Duration::from_secs(10)) }).map_err(|e| e.to_string())?;
        }
        let (chain,handle) = unsafe { self.surface.as_hal::<wgpu::hal::api::Dx12>() }.and_then(|surface|surface.swap_chain().map(|chain|(chain.as_raw() as usize,unsafe{surface.waitable_handle()}))).ok_or("canvas swap chain is not configured")?;
        self.frame_slot.acquire(chain,||{if let Some(handle)=handle && unsafe{WaitForSingleObject(HANDLE(handle.0),10000)}!=WAIT_OBJECT_0{return Err("canvas latency wait failed".into());}Ok(())})
    }
    pub fn present(&mut self,target:wgpu::SurfaceTexture){self.queue.present(target);self.frame_slot.release();}
    pub fn encode(&mut self, samples: &[PenSample], pen_count: u32, frame: u32) -> Result<(wgpu::SurfaceTexture, wgpu::CommandEncoder), String> {
        self.wait()?;
        let floats = [self.config.width as f32, self.config.height as f32];
        let mut data = Vec::with_capacity(16);
        for value in floats { data.extend_from_slice(&value.to_ne_bytes()); }
        for value in [pen_count, frame] { data.extend_from_slice(&value.to_ne_bytes()); }
        self.queue.write_buffer(&self.params, 0, &data);
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            other => return Err(format!("canvas acquire: {other:?}")),
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        DabPass { document: &self.document, buffer: &self.dabs, group: &self.document_group, pipeline: &self.dab_pipeline, size: [self.config.width, self.config.height] }.encode(&mut encoder, &mut self.upload, samples);
        draw(&mut encoder, &self.viewport_view, &self.viewport_pipeline, &self.viewport_group, [3, 1], true, self.timestamps.as_ref().map(crate::cache::GpuTimestamps::pass_writes));
        self.upload.finish_and_recall_on_submit(&encoder);
        Ok((target, encoder))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::util::DeviceExt;

    #[test]
    fn oversized_dab_batch_preserves_first_and_last_document_pixels() {
        let instance = crate::cache::headless_instance();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let params: Vec<u8> = [64.0f32, 64.0].into_iter().flat_map(f32::to_ne_bytes).chain([0u8; 8]).collect();
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("batch params"), contents: &params, usage: wgpu::BufferUsages::UNIFORM });
        let dabs = device.create_buffer(&wgpu::BufferDescriptor { label: Some("batch dabs"), size: 16384 * 16, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 3, resource: dabs.as_entire_binding() },
        ] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(shader_source().into()) });
        let pipeline = pipeline(&device, &layout, &shader, "dab_vertex", "dab_fragment");
        let document = texture(&device, "batch document", 64, 64);
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 64 * 64 * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let outside = PenSample { x: -1000.0, y: -1000.0, pressure: 0.2, ..Default::default() };
        let mut samples = vec![outside; 16385];
        samples[0] = PenSample { x: 16.0, y: 16.0, ..outside };
        samples[16384] = PenSample { x: 48.0, y: 48.0, ..outside };
        let mut upload = wgpu::util::StagingBelt::new(device.clone(), 1 << 20);
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &document.create_view(&Default::default()), depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], ..Default::default() });
        }
        DabPass { document: &document, buffer: &dabs, group: &group, pipeline: &pipeline, size: [64, 64] }.encode(&mut encoder, &mut upload, &samples);
        upload.finish_and_recall_on_submit(&encoder);
        encoder.copy_texture_to_buffer(document.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(64) } }, document.size());
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { let _ = sender.send(result); });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let pixels = readback.slice(..).get_mapped_range().unwrap();
        for (x, y) in [(16, 16), (48, 48)] {
            let offset = (y * 64 + x) * 4;
            assert_eq!(&pixels[offset..offset + 4], &[191, 28, 41, 255], "missing document dab at ({x},{y})");
        }
        assert_eq!(&pixels[..4], &[0, 0, 0, 255]);
    }
}
    #[test]
    fn noop_batches_and_resize_of_the_same_chain_keep_the_acquired_slot_until_present() {
        let mut slot=FrameSlot::default();let waits=std::cell::Cell::new(0);
        let wait=||{waits.set(waits.get()+1);Ok(())};
        slot.acquire(1,wait).unwrap();
        for _ in 0..4 {slot.acquire(1,wait).unwrap();}
        assert_eq!(waits.get(),1);
        slot.release();slot.acquire(1,wait).unwrap();assert_eq!(waits.get(),2);
        slot.acquire(2,wait).unwrap();assert_eq!(waits.get(),3);
    }

//! A bounded GPU loupe. It reads the presenter's existing artwork bindings.
use crate::{GpuRasterError, Uploads};
use layer_render::ColorPickerOverlay;

pub(super) struct Picker {
    pub(super) pipeline: crate::Deferred<wgpu::RenderPipeline>,
    buffer: wgpu::Buffer,
    data: Option<[f32; 16]>,
    dirty: bool,
}
impl Picker {
    pub fn prepare(&self, compiler: &crate::compile::Scope) -> bool {
        if self.data.is_some() {
            compiler.require([&self.pipeline], crate::startup::OTHER)
        } else {
            compiler.warm(&self.pipeline, crate::startup::OTHER, compiler.generation());
            true
        }
    }

    pub fn changed(&self) -> bool { self.dirty && (self.data.is_none() || self.ready()) }

    fn ready(&self) -> bool { self.pipeline.ready() }

    pub fn new(
        device: &crate::PipelineDevice,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
        format: wgpu::TextureFormat,
        shared: &std::sync::OnceLock<crate::Deferred<wgpu::RenderPipeline>>,
    ) -> Self {
        let instance = wgpu::VertexBufferLayout { array_stride: 64, step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4] };
        let pipeline = shared.get_or_init(|| super::present::surface_pipeline(device, "color picker glass", layout, shader, ["picker_vertex", "picker_fragment"],
            Some(instance), format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING))).clone();
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("color picker geometry"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {pipeline, buffer, data: None, dirty: false}
    }
    pub fn set(&mut self, overlay: Option<ColorPickerOverlay>) {
        let data = overlay.map(|o| {
            [
                o.center[0],
                o.center[1],
                o.scale,
                f32::from(o.classic),
                o.sample[0],
                o.sample[1],
                f32::from(o.layer),
                f32::from(o.sample_color_only),
                o.original[0],
                o.original[1],
                o.original[2],
                o.original[3],
                o.candidate[0],
                o.candidate[1],
                o.candidate[2],
                o.candidate[3],
            ]
        });
        if self.data == data {
            return;
        }
        self.data = data;
        self.dirty = true;
    }
    pub fn bounds(&self) -> Option<[f32; 4]> {
        self.data.filter(|_| self.ready()).map(|d| {
            let radius = 48. * d[2];
            [d[0] - radius, d[1] - radius, radius * 2., radius * 2.]
        })
    }
    pub fn upload(
        &mut self,
        uploads: &mut Uploads,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), GpuRasterError> {
        if self.data.is_some() && !self.ready() { return Ok(()); }
        if self.dirty
            && let Some(data) = self.data
        {
            let mut bytes = [0u8; 64];
            for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(data) {
                chunk.copy_from_slice(&value.to_ne_bytes());
            }
            uploads.write(
                encoder,
                &self.buffer,
                &bytes,
            )?;
        }
        self.dirty = false;
        Ok(())
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.data.is_none() || !self.ready() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..6, 0..1);
    }
}

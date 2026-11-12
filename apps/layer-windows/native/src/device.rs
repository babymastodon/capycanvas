//! D3D12 device-removal probe over the shared device watch.
use layer_host::DeviceWatch;

pub(crate) fn removed(device: &wgpu::Device) -> bool {
    unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.is_some_and(|native| {
        unsafe { native.raw_device().GetDeviceRemovedReason() }.is_err()
    })
}

pub(crate) trait D3d12Watch {
    fn is_lost(&self, device: Option<&wgpu::Device>) -> bool;
    fn check(&self) -> Result<(), String>;
}
impl D3d12Watch for DeviceWatch {
    fn is_lost(&self, device: Option<&wgpu::Device>) -> bool {
        // Driver loss can precede the callback, including in idle sibling windows.
        if self.lost().is_none() && device.is_some_and(removed) {
            self.report_lost("The D3D12 device was removed");
        }
        self.lost().is_some()
    }
    fn check(&self) -> Result<(), String> {
        self.error().map_or(Ok(()), |error| Err(error.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{Win32::Graphics::Direct3D12::ID3D12Device5, core::Interface};

    #[test]
    #[ignore = "requires a hardware D3D12 device; removes only this test process's device"]
    fn validation_error_releases_pipeline_and_allows_device_replacement() {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::DX12;
        descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);
        let instance = wgpu::Instance::new(descriptor);
        let (surface, _handle) = crate::host::CapyHost::composition_surface(&instance).unwrap();
        let request = |choice| pollster::block_on(layer_host::gpu::surface_adapter(&instance, &surface, choice, &|_| None));
        let adapter = request(layer_host::gpu::AdapterChoice::Request(wgpu::PowerPreference::HighPerformance)).unwrap();
        let luid = crate::display::adapter_luid(&adapter).unwrap();
        assert!(crate::display::adapter_present(luid).unwrap());
        let same_gpu = |adapter: &wgpu::Adapter| crate::display::adapter_luid(adapter) == Some(luid);
        assert_ne!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: layer_render_wgpu::rendering_limits(adapter.limits()),
            ..Default::default()
        }))
        .unwrap();
        let state = DeviceWatch::observe(&device);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("validation cleanup test"),
            source: wgpu::ShaderSource::Wgsl(
                "@vertex fn vertex() -> @builtin(position) vec4f { return vec4f(0.); }".into(),
            ),
        });
        let invalid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("missing entry point"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("missing"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: None,
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let error = state.check().unwrap_err();
        assert!(error.contains("missing"), "{error}");
        assert_eq!(
            state.check().unwrap_err(),
            error,
            "retain the original error"
        );
        assert!(
            !state.is_lost(Some(&device)),
            "a validation error is not device loss"
        );
        drop(invalid);
        drop(shader);
        assert_eq!(
            instance
                .generate_report()
                .unwrap()
                .hub
                .render_pipelines
                .num_allocated,
            0
        );
        {
            let raw = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
            let native: ID3D12Device5 = raw.raw_device().cast().unwrap();
            unsafe { native.RemoveDevice() };
        }
        assert!(state.is_lost(Some(&device)));
        let _ = device.poll(wgpu::PollType::Poll);
        let held = request(layer_host::gpu::AdapterChoice::Require(&same_gpu));
        assert!(held.is_err(), "removed adapter {luid:?} is still owned; selected {:?}",
            held.as_ref().map(|adapter| (crate::display::adapter_luid(adapter), adapter.get_info())));
        drop(queue);
        drop(device);
        drop(adapter);
        assert!(state.is_lost(None));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(replacement) = request(layer_host::gpu::AdapterChoice::Require(&same_gpu)) {
                assert_eq!(crate::display::adapter_luid(&replacement), Some(luid));
                let (device, _queue) =
                    pollster::block_on(replacement.request_device(&Default::default())).unwrap();
                let clean = DeviceWatch::observe(&device);
                assert!(!clean.is_lost(Some(&device)));
                assert!(clean.check().is_ok());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "removed device is still retained"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

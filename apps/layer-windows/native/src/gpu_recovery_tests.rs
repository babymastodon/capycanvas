//! Shared hardware fixtures. Run removal tests serially in their own process.
use crate::device::D3d12Watch;
use layer_host::{DeviceWatch, Renderer};
use layer_render_wgpu::WgpuRasterizer;
use std::time::{Duration, Instant};
use windows::{Win32::Graphics::Direct3D12::ID3D12Device5, core::Interface};

pub(crate) fn renderer(luid: Option<(u32, i32)>) -> (Renderer, DeviceWatch, (u32, i32)) {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::DX12;
    descriptor.flags.remove(wgpu::InstanceFlags::DEBUG);
    let instance = wgpu::Instance::new(descriptor);
    let (surface, _handle) = crate::host::CapyHost::composition_surface(&instance).unwrap();
    let same_gpu = |adapter: &wgpu::Adapter| crate::display::adapter_luid(adapter) == luid;
    let choice = match luid {
        Some(_) => layer_host::gpu::AdapterChoice::Require(&same_gpu),
        None => layer_host::gpu::AdapterChoice::Request(wgpu::PowerPreference::HighPerformance),
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let adapter = match pollster::block_on(layer_host::gpu::surface_adapter(&instance, &surface, choice, &|_| None)) {
            Ok(adapter) => adapter,
            Err(error) => {
                assert!(Instant::now() < deadline, "{error}");
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        assert_eq!(adapter.get_info().backend, wgpu::Backend::Dx12);
        assert_ne!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
        let luid = crate::display::adapter_luid(&adapter).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: adapter.features()
                & (wgpu::Features::FLOAT32_FILTERABLE
                    | wgpu::Features::FLOAT32_BLENDABLE)
                    | layer_render_wgpu::native_tiles::native_in_place_features(&adapter),
            required_limits: layer_render_wgpu::rendering_limits(adapter.limits()),
            ..Default::default()
        }))
        .unwrap();
        let state = DeviceWatch::observe(&device);
        let gpu =
            WgpuRasterizer::from_wgpu_native_staged(adapter, device, queue, Default::default())
                .unwrap();
        assert!(!state.is_lost(Some(gpu.device())));
        state.check().unwrap();
        return (Renderer(Some(gpu.into())), state, luid);
    }
}

pub(crate) fn remove_device(renderer: &Renderer, state: &DeviceWatch) {
    let gpu = renderer.0.as_ref().unwrap();
    {
        let native = unsafe { gpu.device().as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
        let device: ID3D12Device5 = native.raw_device().cast().unwrap();
        unsafe { device.RemoveDevice() };
    }
    let _ = gpu.device().poll(wgpu::PollType::Poll);
    assert!(state.is_lost(Some(gpu.device())));
}

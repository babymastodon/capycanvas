use super::*;
use std::{sync::mpsc, time::Duration};
use windows::{core::Interface, Win32::{Foundation::{HANDLE, WAIT_OBJECT_0}, Graphics::{Direct3D12::{ID3D12Fence, D3D12_FENCE_FLAG_NONE}, DirectComposition::{DCompositionCreateDevice, IDCompositionDevice}, Dxgi::{IDXGISwapChain3, DXGI_PRESENT}}, System::{Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED}, Threading::WaitForSingleObject}}};

struct Com;
impl Drop for Com {fn drop(&mut self){unsafe{CoUninitialize()};}}
struct FenceGate {hold:ID3D12Fence,chain:IDXGISwapChain3}
impl Drop for FenceGate {
    fn drop(&mut self) {
        unsafe {
            let _=self.hold.Signal(1);
            let _=self.chain.SetMaximumFrameLatency(2);
            let _=self.chain.Present(0,DXGI_PRESENT::default()).ok();
        }
    }
}
fn acquire(surface:&wgpu::Surface<'_>)->wgpu::SurfaceTexture {
    match surface.get_current_texture(){wgpu::CurrentSurfaceTexture::Success(frame)|wgpu::CurrentSurfaceTexture::Suboptimal(frame)=>frame,other=>panic!("acquire second surface: {other:?}")}
}
#[test]
fn canvas_latency_wait_does_not_hold_another_surface_present_behind_its_gpu_fence() {
    unsafe{CoInitializeEx(None,COINIT_MULTITHREADED).ok().unwrap()};let _com=Com;
    let composition:IDCompositionDevice=unsafe{DCompositionCreateDevice(None)}.unwrap();
    let canvas_visual=unsafe{composition.CreateVisual()}.unwrap();
    let ui_visual=unsafe{composition.CreateVisual()}.unwrap();
    let instance=Gpu::instance();
    let canvas=unsafe{instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(canvas_visual.as_raw()))}.unwrap();
    let ui=unsafe{instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(ui_visual.as_raw()))}.unwrap();
    let mut gpu=Gpu::new(instance,canvas,64,64,1.0).unwrap();
    ui.configure(&gpu.device,&gpu.config);
    let frame=acquire(&ui);
    let mut encoder=gpu.device.create_command_encoder(&Default::default());
    {
        let _clear=encoder.begin_render_pass(&wgpu::RenderPassDescriptor {color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:&frame.texture.create_view(&Default::default()),depth_slice:None,resolve_target:None,ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::GREEN),store:wgpu::StoreOp::Store}})],..Default::default()});
    }
    gpu.queue.submit([encoder.finish()]);
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let native=unsafe{gpu.device.as_hal::<wgpu::hal::api::Dx12>()}.unwrap();
    let hold:ID3D12Fence=unsafe{native.raw_device().CreateFence(0,D3D12_FENCE_FLAG_NONE)}.unwrap();
    let completed:ID3D12Fence=unsafe{native.raw_device().CreateFence(0,D3D12_FENCE_FLAG_NONE)}.unwrap();
    drop(native);
    let canvas_native=unsafe{gpu.surface.as_hal::<wgpu::hal::api::Dx12>()}.unwrap();
    let chain=canvas_native.swap_chain().unwrap();
    let latency=unsafe{canvas_native.waitable_handle()}.expect("actual canvas latency event");
    assert_eq!(unsafe{WaitForSingleObject(HANDLE(latency.0),0)},WAIT_OBJECT_0,"initial canvas latency permit");
    drop(canvas_native);
    let queue=gpu.queue.clone();
    std::thread::scope(|scope| {
        let gate=FenceGate {hold,chain};
        let native_queue=unsafe{queue.as_hal::<wgpu::hal::api::Dx12>()}.unwrap();
        unsafe{native_queue.as_raw().Wait(&gate.hold,1)}.unwrap();
        drop(native_queue);
        queue.submit([gpu.device.create_command_encoder(&Default::default()).finish()]);
        let native_queue=unsafe{queue.as_hal::<wgpu::hal::api::Dx12>()}.unwrap();
        unsafe{native_queue.as_raw().Signal(&completed,1)}.unwrap();drop(native_queue);
        let (entered_tx,entered_rx)=mpsc::channel();let (wait_tx,wait_rx)=mpsc::channel();
        let canvas_thread=scope.spawn(move||{entered_tx.send(()).unwrap();let result=gpu.wait();let _=wait_tx.send(result);gpu});
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let pending_before_present=matches!(wait_rx.recv_timeout(Duration::from_millis(100)),Err(mpsc::RecvTimeoutError::Timeout));
        let (present_tx,present_rx)=mpsc::channel();
        let ui_thread=scope.spawn(move||{queue.present(frame);present_tx.send(()).unwrap();});
        let presented=present_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        let canvas_fence_pending=unsafe{completed.GetCompletedValue()}==0;
        let canvas_wait_pending=matches!(wait_rx.try_recv(),Err(mpsc::TryRecvError::Empty));
        drop(gate);
        let wait_result=wait_rx.recv_timeout(Duration::from_secs(2));
        let gpu=canvas_thread.join().unwrap();ui_thread.join().unwrap();
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(matches!(wait_result,Ok(Ok(()))),"canvas latency wait failed to complete after gate release: {wait_result:?}");
        assert!(pending_before_present,"canvas must be inside its actual latency wait");
        assert!(presented,"second-surface CPU present waited for the canvas GPU fence");
        assert!(canvas_fence_pending,"canvas GPU work completed before the independent present check");
        assert!(canvas_wait_pending,"canvas latency wait ended before the independent present check");
    });
}

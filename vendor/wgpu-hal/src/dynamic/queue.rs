use alloc::{boxed::Box, vec::Vec};

use crate::{
    DeviceError, DynCommandBuffer, DynFence, DynResource, DynSurface, DynSurfaceTexture,
    FenceValue, Queue, SurfaceError,
};

use super::DynResourceExt as _;

pub trait DynQueue: DynResource {
    unsafe fn submit(
        &self,
        command_buffers: &[&dyn DynCommandBuffer],
        surface_textures: &[&dyn DynSurfaceTexture],
        signal_fence: (&dyn DynFence, FenceValue),
    ) -> Result<(), DeviceError>;
    fn supports_nonblocking_present(&self, _surface: &dyn DynSurface) -> bool { false }

    unsafe fn try_present(
        &self,
        _surface: &dyn DynSurface,
        _texture: &dyn DynSurfaceTexture,
    ) -> Result<crate::PresentAttempt, SurfaceError> { Ok(crate::PresentAttempt::Unsupported) }
    unsafe fn present(
        &self,
        surface: &dyn DynSurface,
        texture: Box<dyn DynSurfaceTexture>,
    ) -> Result<(), SurfaceError>;
    unsafe fn get_timestamp_period(&self) -> f32;
    unsafe fn wait_for_idle(&self) -> Result<(), DeviceError>;
}

impl<Q: Queue + DynResource> DynQueue for Q {
    unsafe fn submit(
        &self,
        command_buffers: &[&dyn DynCommandBuffer],
        surface_textures: &[&dyn DynSurfaceTexture],
        signal_fence: (&dyn DynFence, FenceValue),
    ) -> Result<(), DeviceError> {
        let command_buffers = command_buffers
            .iter()
            .map(|cb| (*cb).expect_downcast_ref())
            .collect::<Vec<_>>();
        let surface_textures = surface_textures
            .iter()
            .map(|surface| (*surface).expect_downcast_ref())
            .collect::<Vec<_>>();
        let signal_fence = (signal_fence.0.expect_downcast_ref(), signal_fence.1);
        unsafe { Q::submit(self, &command_buffers, &surface_textures, signal_fence) }
    }

    fn supports_nonblocking_present(&self, surface: &dyn DynSurface) -> bool {
        Q::supports_nonblocking_present(self, surface.expect_downcast_ref())
    }

    unsafe fn try_present(
        &self,
        surface: &dyn DynSurface,
        texture: &dyn DynSurfaceTexture,
    ) -> Result<crate::PresentAttempt, SurfaceError> {
        unsafe { Q::try_present(self, surface.expect_downcast_ref(), texture.expect_downcast_ref()) }
    }

    unsafe fn present(
        &self,
        surface: &dyn DynSurface,
        texture: Box<dyn DynSurfaceTexture>,
    ) -> Result<(), SurfaceError> {
        let surface = surface.expect_downcast_ref();
        unsafe { Q::present(self, surface, texture.unbox()) }
    }

    unsafe fn get_timestamp_period(&self) -> f32 {
        unsafe { Q::get_timestamp_period(self) }
    }

    unsafe fn wait_for_idle(&self) -> Result<(), DeviceError> {
        unsafe { Q::wait_for_idle(self) }
    }
}

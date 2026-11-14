/*! Presentation.

## Lifecycle

Whenever a submission detects the use of any surface texture, it adds it to the device
tracker for the duration of the submission (temporarily, while recording).
It's added with `UNINITIALIZED` state and transitioned into `empty()` state.
When this texture is presented, we remove it from the device tracker as well as
extract it from the hub.
!*/

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use core::mem::ManuallyDrop;

#[cfg(feature = "trace")]
use crate::device::trace::{Action, IntoTrace};
use crate::{
    conv,
    device::{queue::Queue, Device, DeviceError, MissingDownlevelFlags, WaitIdleError},
    global::Global,
    hal_label, id,
    instance::Surface,
    resource::{self, Labeled},
};

use thiserror::Error;
use wgt::{
    error::{ErrorType, WebGpuError},
    SurfaceStatus as Status,
};

const FRAME_TIMEOUT_MS: u32 = 1000;

#[derive(Debug)]
pub(crate) struct Presentation {
    pub(crate) device: Arc<Device>,
    pub(crate) config: wgt::SurfaceConfiguration<Vec<wgt::TextureFormat>>,
    pub(crate) acquired_texture: Option<Arc<resource::Texture>>,
    pub(crate) prepared: bool,
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum SurfaceError {
    #[error("Surface is invalid")]
    Invalid,
    #[error("Surface is not configured for presentation")]
    NotConfigured,
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("Surface image is already acquired")]
    AlreadyAcquired,
    #[error("No surface image is currently acquired to present")]
    NothingToPresent,
    #[error("Texture has been destroyed")]
    TextureDestroyed,
}

impl WebGpuError for SurfaceError {
    fn webgpu_error_type(&self) -> ErrorType {
        match self {
            Self::Device(e) => e.webgpu_error_type(),
            Self::Invalid
            | Self::NotConfigured
            | Self::AlreadyAcquired
            | Self::NothingToPresent
            | Self::TextureDestroyed => ErrorType::Validation,
        }
    }
}

#[derive(Clone, Debug, Error)]
#[non_exhaustive]
pub enum ConfigureSurfaceError {
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error("Invalid surface")]
    InvalidSurface,
    #[error("The view format {0:?} is not compatible with texture format {1:?}, only changing srgb-ness is allowed.")]
    InvalidViewFormat(wgt::TextureFormat, wgt::TextureFormat),
    #[error(transparent)]
    MissingDownlevelFlags(#[from] MissingDownlevelFlags),
    #[error("The `SurfaceOutput` returned by `get_current_texture` must be dropped before re-configuring via `configure` or  retrieving a new texture via `get_current_texture`.")]
    PreviousOutputExists,
    #[error("Failed to wait for GPU to come idle before reconfiguring the Surface")]
    GpuWaitTimeout,
    #[error("Both `Surface` width and height must be non-zero. Wait to recreate the `Surface` until the window has non-zero area.")]
    ZeroArea,
    #[error("`Surface` width and height must be within the maximum supported texture size. Requested was ({width}, {height}), maximum extent for either dimension is {max_texture_dimension_2d}.")]
    TooLarge {
        width: u32,
        height: u32,
        max_texture_dimension_2d: u32,
    },
    #[error("Surface does not support the adapter's queue family")]
    UnsupportedQueueFamily,
    #[error("Requested format {requested:?} is not in list of supported formats: {available:?}")]
    UnsupportedFormat {
        requested: wgt::TextureFormat,
        available: Vec<wgt::TextureFormat>,
    },
    #[error("Requested color space {requested:?} is not in the list of color spaces supported for format {format:?}: {available:?}")]
    UnsupportedColorSpace {
        requested: wgt::SurfaceColorSpace,
        format: wgt::TextureFormat,
        available: wgt::SurfaceColorSpaces,
    },
    #[error("Requested present mode {requested:?} is not in the list of supported present modes: {available:?}")]
    UnsupportedPresentMode {
        requested: wgt::PresentMode,
        available: Vec<wgt::PresentMode>,
    },
    #[error("Requested alpha mode {requested:?} is not in the list of supported alpha modes: {available:?}")]
    UnsupportedAlphaMode {
        requested: wgt::CompositeAlphaMode,
        available: Vec<wgt::CompositeAlphaMode>,
    },
    #[error("Requested usage {requested:?} is not in the list of supported usages: {available:?}")]
    UnsupportedUsage {
        requested: wgt::TextureUses,
        available: wgt::TextureUses,
    },
}

impl From<WaitIdleError> for ConfigureSurfaceError {
    fn from(e: WaitIdleError) -> Self {
        match e {
            WaitIdleError::Device(d) => ConfigureSurfaceError::Device(d),
            WaitIdleError::WrongSubmissionIndex(..) => unreachable!(),
            WaitIdleError::Timeout => ConfigureSurfaceError::GpuWaitTimeout,
        }
    }
}

impl WebGpuError for ConfigureSurfaceError {
    fn webgpu_error_type(&self) -> ErrorType {
        match self {
            Self::Device(e) => e.webgpu_error_type(),
            Self::MissingDownlevelFlags(e) => e.webgpu_error_type(),
            Self::InvalidSurface
            | Self::InvalidViewFormat(..)
            | Self::PreviousOutputExists
            | Self::GpuWaitTimeout
            | Self::ZeroArea
            | Self::TooLarge { .. }
            | Self::UnsupportedQueueFamily
            | Self::UnsupportedFormat { .. }
            | Self::UnsupportedColorSpace { .. }
            | Self::UnsupportedPresentMode { .. }
            | Self::UnsupportedAlphaMode { .. }
            | Self::UnsupportedUsage { .. } => ErrorType::Validation,
        }
    }
}

pub type ResolvedSurfaceOutput = SurfaceOutput<Arc<resource::Texture>>;

#[repr(C)]
#[derive(Debug)]
pub struct SurfaceOutput<T = id::TextureId> {
    pub status: Status,
    pub texture: Option<T>,
}

impl Surface {
    pub fn get_current_texture(&self) -> Result<ResolvedSurfaceOutput, SurfaceError> {
        profiling::scope!("Surface::get_current_texture");

        let mut presentation = self.presentation.lock();
        let (device, config) = if let Some(ref present) = *presentation {
            present.device.check_is_valid()?;
            if present.acquired_texture.is_some() {
                return Err(SurfaceError::AlreadyAcquired);
            }
            (present.device.clone(), present.config.clone())
        } else {
            return Err(SurfaceError::NotConfigured);
        };

        let suf = self.raw(device.backend()).unwrap();
        let (texture, status) = match unsafe {
            suf.acquire_texture(
                Some(core::time::Duration::from_millis(FRAME_TIMEOUT_MS as u64)),
                device.fence.as_ref(),
            )
        } {
            Ok(ast) => {
                let texture_desc = wgt::TextureDescriptor {
                    label: hal_label(
                        Some(alloc::borrow::Cow::Borrowed("<Surface Texture>")),
                        device.instance_flags,
                    ),
                    size: wgt::Extent3d {
                        width: config.width,
                        height: config.height,
                        depth_or_array_layers: 1,
                    },
                    sample_count: 1,
                    mip_level_count: 1,
                    format: config.format,
                    dimension: wgt::TextureDimension::D2,
                    usage: config.usage,
                    view_formats: config.view_formats,
                };
                let format_features = wgt::TextureFormatFeatures {
                    allowed_usages: wgt::TextureUsages::RENDER_ATTACHMENT,
                    flags: wgt::TextureFormatFeatureFlags::MULTISAMPLE_X4
                        | wgt::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE,
                };
                let hal_usage = conv::map_texture_usage(
                    config.usage,
                    config.format.into(),
                    format_features.flags,
                );
                let clear_view_desc = hal::TextureViewDescriptor {
                    label: hal_label(
                        Some("(wgpu internal) clear surface texture view"),
                        device.instance_flags,
                    ),
                    format: config.format,
                    dimension: wgt::TextureViewDimension::D2,
                    usage: wgt::TextureUses::COLOR_TARGET,
                    range: wgt::ImageSubresourceRange::default(),
                };
                let clear_view = match unsafe {
                    device
                        .raw()
                        .create_texture_view(ast.texture.as_ref().borrow(), &clear_view_desc)
                } {
                    Ok(view) => view,
                    Err(error) => {
                        drop(presentation);
                        return Err(device.handle_hal_error(error).into());
                    }
                };

                let present = presentation.as_mut().unwrap();
                let needs_initialization = !ast.texture.retains_initialized_contents();
                let texture = resource::Texture::new(
                    &device,
                    resource::TextureInner::Surface { raw: ast.texture },
                    hal_usage,
                    &texture_desc,
                    format_features,
                    resource::TextureClearMode::Surface {
                        clear_view: ManuallyDrop::new(clear_view),
                    },
                    needs_initialization,
                );

                let texture = Arc::new(texture);

                device
                    .trackers
                    .lock()
                    .textures
                    .insert_single(&texture, wgt::TextureUses::UNINITIALIZED);

                present.acquired_texture = Some(texture.clone());
                present.prepared = false;

                let status = if ast.suboptimal {
                    Status::Suboptimal
                } else {
                    Status::Good
                };
                (Some(texture), status)
            }
            Err(err) => (
                None,
                match err {
                    hal::SurfaceError::Timeout => Status::Timeout,
                    hal::SurfaceError::Occluded => Status::Occluded,
                    hal::SurfaceError::Lost => Status::Lost,
                    hal::SurfaceError::Device(err) => {
                        drop(presentation);
                        return Err(device.handle_hal_error(err).into());
                    }
                    hal::SurfaceError::Outdated => Status::Outdated,
                    hal::SurfaceError::Other(msg) => {
                        log::error!("acquire error: {msg}");
                        Status::Lost
                    }
                },
            ),
        };

        Ok(ResolvedSurfaceOutput { status, texture })
    }

    pub fn present(&self) -> Result<Status, SurfaceError> {
        self.present_inner(false).map(Option::unwrap)
    }

    fn present_inner(&self, defer: bool) -> Result<Option<Status>, SurfaceError> {
        let (queue, texture) = {
            let presentation = self.presentation.lock();
            let present = presentation.as_ref().ok_or(SurfaceError::NotConfigured)?;
            (present.device.check_is_valid().and_then(|_| present.device.get_queue().ok_or(DeviceError::Lost)),
                present.acquired_texture.clone())
        };
        match queue {
            Ok(queue) => queue.present_inner(self, defer),
            Err(error) => {
                if let Some(texture) = texture { self.release_current(&texture); }
                Err(SurfaceError::from(error))
            }
        }
    }
}

impl Queue {
    pub fn present(&self, surface: &Surface) -> Result<Status, SurfaceError> {
        self.present_inner(surface, false).map(Option::unwrap)
    }

    pub fn try_present(&self, surface: &Surface) -> Result<Option<Status>, SurfaceError> {
        self.present_inner(surface, true)
    }

    fn presentation_ready(&self, indices: &crate::device::resource::CommandIndices) -> Result<bool, hal::DeviceError> {
        unsafe { self.device.raw().get_fence_value(self.device.fence.as_ref()) }
            .map(|completed| completed >= indices.last_non_presentation_submission_index)
    }

    fn present_inner(&self, surface: &Surface, defer: bool) -> Result<Option<Status>, SurfaceError> {
        profiling::scope!("Queue::present");
        let device = &self.device;
        let texture = {
            let presentation = surface.presentation.lock();
            let present = presentation.as_ref().ok_or(SurfaceError::NotConfigured)?;
            if !Arc::ptr_eq(&present.device, device) {
                return Err(SurfaceError::Device(DeviceError::DeviceMismatch(Box::new(
                    crate::device::DeviceMismatch {
                        res: self.error_ident(), res_device: device.error_ident(),
                        target: None, target_device: present.device.error_ident(),
                    },
                ))));
            }
            present.acquired_texture.clone().ok_or(SurfaceError::NothingToPresent)?
        };
        if defer {
            let ready = {
                let indices = device.command_indices.read();
                self.presentation_ready(&indices)
            }.map_err(|error| {
                surface.release_current(&texture);
                SurfaceError::from(device.handle_hal_error(error))
            })?;
            if !ready { return Ok(None); }
        }
        let nonblocking = defer && self.raw().supports_nonblocking_present(surface.raw(device.backend()).unwrap());
        let prepared = nonblocking && surface.presentation.lock().as_ref().is_some_and(|present|
            present.prepared && present.acquired_texture.as_ref().is_some_and(|current| Arc::ptr_eq(current, &texture)));
        if !prepared {
            self.prepare_surface_texture_for_present(&texture).map_err(|error| {
                surface.release_current(&texture);
                error
            })?;
            if nonblocking {
                let mut presentation = surface.presentation.lock();
                let present = presentation.as_mut().ok_or(SurfaceError::NotConfigured)?;
                if present.acquired_texture.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &texture)) {
                    return Err(SurfaceError::NothingToPresent);
                }
                present.prepared = true;
            }
        }
        let result = {
            let mut presentation = surface.presentation.lock();
            let present = presentation.as_mut().ok_or(SurfaceError::NotConfigured)?;
            if !Arc::ptr_eq(&present.device, device)
                || present.acquired_texture.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &texture)) {
                return Err(SurfaceError::NothingToPresent);
            }
            let attempt = if nonblocking {
                let snatch = device.snatchable_lock.read();
                let indices = device.command_indices.write();
                match self.presentation_ready(&indices) {
                    Ok(false) => return Ok(None),
                    Err(error) => Err(hal::SurfaceError::Device(error)),
                    Ok(true) => match texture.inner.get(&snatch).maybe_valid() {
                        Some(resource::TextureInner::Surface { raw }) => unsafe {
                            self.raw().try_present(surface.raw(device.backend()).unwrap(), raw.as_ref())
                        },
                        _ => return Err(SurfaceError::TextureDestroyed),
                    },
                }
            } else { Ok(hal::PresentAttempt::Unsupported) };
            if matches!(attempt, Ok(hal::PresentAttempt::Pending)) { return Ok(None); }
            if !matches!(attempt, Ok(hal::PresentAttempt::Unsupported)) {
                let mut snatch = device.snatchable_lock.write();
                present.acquired_texture.take();
                let raw = texture.inner.snatch(&mut snatch).maybe_valid();
                match attempt {
                    Ok(hal::PresentAttempt::Presented) => Ok(()),
                    Err(error) => {
                        if let Some(resource::TextureInner::Surface { raw }) = raw {
                            unsafe { surface.raw(device.backend()).unwrap().discard_texture(raw); }
                        }
                        Err(error)
                    },
                    _ => unreachable!(),
                }
            } else {
            let mut snatch = device.snatchable_lock.write();
            let indices = device.command_indices.write();
            let ready = if defer {
                self.presentation_ready(&indices)
            } else { Ok(true) };
            match ready {
                Err(error) => {
                    present.acquired_texture.take();
                    texture.inner.snatch(&mut snatch);
                    Err(hal::SurfaceError::Device(error))
                },
                Ok(false) => return Ok(None),
                Ok(true) => {
                    present.acquired_texture.take();
                    match texture.inner.snatch(&mut snatch).maybe_valid() {
                        None => return Err(SurfaceError::TextureDestroyed),
                        Some(resource::TextureInner::Surface { raw }) => {
                            let raw_surface = surface.raw(device.backend()).unwrap();
                            unsafe { self.raw().present(raw_surface, raw) }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            }
        };
        match result {
            Ok(()) => Ok(Some(Status::Good)),
            Err(err) => match err {
                hal::SurfaceError::Timeout => Ok(Some(Status::Timeout)),
                hal::SurfaceError::Occluded => Ok(Some(Status::Occluded)),
                hal::SurfaceError::Lost => Ok(Some(Status::Lost)),
                hal::SurfaceError::Device(err) => {
                    Err(SurfaceError::from(device.handle_hal_error(err)))
                }
                hal::SurfaceError::Outdated => Ok(Some(Status::Outdated)),
                hal::SurfaceError::Other(msg) => {
                    log::error!("present error: {msg}");
                    Err(SurfaceError::Invalid)
                }
            },
        }
    }
}

impl Surface {
    #[cfg(feature = "trace")]
    fn record(&self, action: Action<'_, crate::command::PointerReferences>) {
        let device = self.presentation.lock().as_ref().map(|present| present.device.clone());
        if let Some(device) = device {
            if let Some(ref mut trace) = *device.trace.lock() { trace.add(action); }
        }
    }

    fn release_current(&self, texture: &Arc<resource::Texture>) {
        let mut presentation = self.presentation.lock();
        if let Some(present) = presentation.as_mut() {
            if present.acquired_texture.as_ref().is_some_and(|current| Arc::ptr_eq(current, texture)) {
                present.acquired_texture.take();
                texture.inner.snatch(&mut present.device.snatchable_lock.write());
            }
        }
    }

    pub fn discard(&self) -> Result<(), SurfaceError> {
        profiling::scope!("Surface::discard");

        let mut presentation = self.presentation.lock();
        let present = match presentation.as_mut() {
            Some(present) => present,
            None => return Err(SurfaceError::NotConfigured),
        };

        let device = &present.device;

        let texture = present
            .acquired_texture
            .take()
            .ok_or(SurfaceError::NothingToPresent)?;

        let mut exclusive_snatch_guard = device.snatchable_lock.write();
        let inner = texture
            .inner
            .snatch(&mut exclusive_snatch_guard)
            .maybe_valid();
        drop(exclusive_snatch_guard);

        device.check_is_valid()?;

        match inner {
            None => return Err(SurfaceError::TextureDestroyed),
            Some(resource::TextureInner::Surface { raw }) => {
                let raw_surface = self.raw(device.backend()).unwrap();
                unsafe { raw_surface.discard_texture(raw) };
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    /// Like `discard`, drops the inner texture reference, but skips the
    /// HAL `discard_texture` call. Safe to call during unwinding
    pub fn release(&self) -> Result<(), SurfaceError> {
        profiling::scope!("Surface::release");

        let mut presentation = self.presentation.lock();
        let Some(present) = presentation.as_mut() else {
            return Err(SurfaceError::NotConfigured);
        };

        // `texture` is dropped here, decrementing the refcount of
        // Arc<SwapchainAcquireSemaphore>. If this was the last Arc, the Texture
        // is freed, which drops NativeSurfaceTextureMetadata and
        // its Arc<SwapchainAcquireSemaphore>.
        _ = present
            .acquired_texture
            .take()
            .ok_or(SurfaceError::NothingToPresent)?;

        Ok(())
    }
}

impl Global {
    pub fn surface_get_current_texture(
        &self,
        surface_id: id::SurfaceId,
        texture_id_in: Option<id::TextureId>,
    ) -> Result<SurfaceOutput, SurfaceError> {
        let surface = self.surfaces.get(surface_id);

        let fid = self.hub.textures.prepare(texture_id_in);

        let output = surface.get_current_texture()?;

        #[cfg(feature = "trace")]
        if let Some(texture) = output.texture.as_ref() {
            if let Some(ref mut trace) = *texture.device.trace.lock() {
                trace.add(Action::GetSurfaceTexture { id: texture.to_trace(), parent: surface.to_trace() });
            }
        }

        let status = output.status;
        let texture_id = output.texture.map(|texture| fid.assign(texture));

        Ok(SurfaceOutput {
            status,
            texture: texture_id,
        })
    }

    pub fn surface_present(&self, surface_id: id::SurfaceId) -> Result<Status, SurfaceError> {
        let surface = self.surfaces.get(surface_id);

        #[cfg(feature = "trace")]
        surface.record(Action::Present(surface.to_trace()));

        surface.present()
    }

    pub fn surface_try_present(&self, surface_id: id::SurfaceId) -> Result<Option<Status>, SurfaceError> {
        let surface = self.surfaces.get(surface_id);
        let status = surface.present_inner(true)?;
        #[cfg(feature = "trace")]
        if status.is_some() { surface.record(Action::Present(surface.to_trace())); }
        Ok(status)
    }

    pub fn surface_texture_discard(&self, surface_id: id::SurfaceId) -> Result<(), SurfaceError> {
        let surface = self.surfaces.get(surface_id);

        #[cfg(feature = "trace")]
        surface.record(Action::DiscardSurfaceTexture(surface.to_trace()));

        surface.discard()
    }

    pub fn surface_texture_release(&self, surface_id: id::SurfaceId) -> Result<(), SurfaceError> {
        let surface = self.surfaces.get(surface_id);

        #[cfg(feature = "trace")]
        surface.record(Action::ReleaseSurfaceTexture(surface.to_trace()));

        surface.release()
    }
}

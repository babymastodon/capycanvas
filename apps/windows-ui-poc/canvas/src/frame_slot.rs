#[derive(Default)]
pub struct FrameSlot {
    reserved: bool,
    chain: usize,
}
impl FrameSlot {
    fn acquire(&mut self, chain: usize, wait: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
        if self.chain != chain {
            self.chain = chain;
            self.reserved = false;
        }
        if !self.reserved {
            wait()?;
            self.reserved = true;
        }
        Ok(())
    }
    fn release(&mut self) {
        self.reserved = false;
    }
    #[cfg(windows)]
    pub fn wait(&mut self, surface: &wgpu::Surface<'_>) -> Result<(), String> {
        use windows::{core::Interface, Win32::{Foundation::{HANDLE, WAIT_OBJECT_0}, System::Threading::WaitForSingleObject}};
        let (chain, handle) = unsafe { surface.as_hal::<wgpu::hal::api::Dx12>() }
            .and_then(|surface| surface.swap_chain().map(|chain| (chain.as_raw() as usize, unsafe { surface.waitable_handle() })))
            .ok_or("frame swap chain is not configured")?;
        self.acquire(chain, || {
            if let Some(handle) = handle && unsafe { WaitForSingleObject(HANDLE(handle.0), 10000) } != WAIT_OBJECT_0 {
                return Err("frame latency wait failed".into());
            }
            Ok(())
        })
    }
    #[cfg(windows)]
    pub fn present(&mut self, queue: &wgpu::Queue, target: wgpu::SurfaceTexture) {
        queue.present(target);
        self.release();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_surfaces_keep_noop_and_same_chain_resize_reservations_until_their_own_present() {
        let mut upper = FrameSlot::default();
        let mut popup = FrameSlot::default();
        let waits = std::cell::Cell::new(0);
        let wait = || { waits.set(waits.get() + 1); Ok(()) };
        upper.acquire(1, wait).unwrap();
        popup.acquire(2, wait).unwrap();
        for _ in 0..4 {
            upper.acquire(1, wait).unwrap();
            popup.acquire(2, wait).unwrap();
        }
        assert_eq!(waits.get(), 2);
        upper.release();
        upper.acquire(1, wait).unwrap();
        popup.acquire(2, wait).unwrap();
        assert_eq!(waits.get(), 3);
        popup.acquire(3, wait).unwrap();
        assert_eq!(waits.get(), 4);
        upper.acquire(1, wait).unwrap();
        assert_eq!(waits.get(), 4);
    }
    #[test]
    fn failed_wait_does_not_reserve_the_new_chain() {
        let mut slot = FrameSlot::default();
        slot.acquire(1, || Ok(())).unwrap();
        assert_eq!(slot.acquire(2, || Err("wait failed".into())), Err("wait failed".into()));
        assert!(!slot.reserved);
        let waits = std::cell::Cell::new(0);
        slot.acquire(2, || { waits.set(waits.get() + 1); Ok(()) }).unwrap();
        assert_eq!(waits.get(), 1);
        assert!(slot.reserved);
    }
}

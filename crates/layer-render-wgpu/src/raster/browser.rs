//! GPU objects stay on their browser event loop; only packed bytes cross to the
//! host's compression worker. One consumer bounds cached scratch per renderer.
use super::*;
use std::{cell::RefCell, future::Future, pin::Pin, rc::Rc, task::Waker};

pub type BrowserRasterEncoder = Rc<
    dyn Fn(
        Vec<u8>,
        Vec<layer_core::color::PixelDescriptor>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TileBlob>, String>>>>,
>;

pub(super) struct CaptureWorker {
    sender: Option<mpsc::SyncSender<NativeCapture>>,
    pending: Arc<AtomicUsize>,
    pub(super) staging: Arc<AtomicU64>,
    pub(super) error: Arc<std::sync::Mutex<Option<String>>>,
    wake: Rc<RefCell<Option<Waker>>>,
}
impl CaptureWorker {
    pub(super) fn new(
        encoder: BrowserRasterEncoder,
        completion:Arc<PreparationWake>,
    ) -> Result<Self, GpuRasterError> {
        let (sender, receiver) = mpsc::sync_channel::<NativeCapture>(16);
        let pending = Arc::new(AtomicUsize::new(0));
        let staging = Arc::new(AtomicU64::new(0));
        let error = Arc::new(std::sync::Mutex::new(None));
        let wake: Rc<RefCell<Option<Waker>>> = Rc::default();
        let (count, bytes, failure, signal) = (
            pending.clone(),
            staging.clone(),
            error.clone(),
            wake.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let captures = std::future::poll_fn(|cx| match receiver.try_recv() {
                    Ok(job) => std::task::Poll::Ready(Some(job)),
                    Err(mpsc::TryRecvError::Disconnected) => std::task::Poll::Ready(None),
                    Err(mpsc::TryRecvError::Empty) => {
                        *signal.borrow_mut() = Some(cx.waker().clone());
                        std::task::Poll::Pending
                    }
                })
                .await;
                let Some(captures) = captures else { break };
                let size = captures.storage_bytes();
                if failure.lock().unwrap().is_some() {
                    drop(captures);
                } else if let Err(message) = captures.finish_browser(&encoder).await {
                    *failure.lock().unwrap() = Some(message);
                }
                bytes.fetch_sub(size, Ordering::Release);
                count.fetch_sub(1, Ordering::Release);
                std::task::Wake::wake_by_ref(&completion);
            }
        });
        Ok(Self {
            sender: Some(sender),
            pending,
            staging,
            error,
            wake,
        })
    }
    pub(super) fn ready(&self) -> bool {
        self.pending.load(Ordering::Acquire) < 16
            && self.staging.load(Ordering::Acquire) <= MAX_CAPTURE_BYTES - CAPTURE_CHUNK
    }
    pub(super) fn submit_batch(&self, captures: NativeCapture) -> Result<(), GpuRasterError> {
        let size = captures.storage_bytes();
        self.pending.fetch_add(1, Ordering::Release);
        self.staging.fetch_add(size, Ordering::Release);
        if self.sender.as_ref().unwrap().try_send(captures).is_err() {
            self.pending.fetch_sub(1, Ordering::Release);
            self.staging.fetch_sub(size, Ordering::Release);
            return Err(GpuRasterError::Effect(
                "Browser raster queue is full or stopped".into(),
            ));
        }
        if let Some(waker) = self.wake.borrow_mut().take() {
            waker.wake();
        }
        Ok(())
    }
}
impl Drop for CaptureWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(waker) = self.wake.borrow_mut().take() {
            waker.wake();
        }
    }
}

impl RasterCapture {
    #[cfg(test)]
    pub(super) async fn finish_browser(self,encoder:&BrowserRasterEncoder)->Result<(),String> {
        let pool=self.pool.clone();self.finish_browser_recycling(encoder,move|buffer|pool.put(buffer)).await
    }
    pub(super) async fn finish_browser_recycling(mut self,encoder:&BrowserRasterEncoder,recycle:impl Fn(wgpu::Buffer))->Result<(),String> {
        let result: Result<(), String> = async {
            for chunk in &mut self.chunks {
                (&mut chunk.ready)
                    .await
                    .map_err(|_| "Raster mapping was abandoned")??;
                let size = chunk.entries.iter().map(|e| e.size).sum::<u64>();
                let bytes = {
                    let mapped = chunk
                        .buffer
                        .get_mapped_range(..)
                        .map_err(|e| e.to_string())?;
                    let mut bytes=Vec::with_capacity(size as usize);
                    let mut offset=0;
                    for entry in &chunk.entries {
                        if let Some((at,comparison))=&entry.comparison {comparison.resolve(&mapped[*at as usize..*at as usize+crate::native_tiles::comparison::RESULT_BYTES as usize])?;}
                        bytes.extend_from_slice(&mapped[offset..offset+entry.size as usize]);
                        offset+=entry.size as usize;
                    }
                    bytes
                };
                chunk.buffer.unmap();
                recycle(chunk.buffer.clone());
                let _scratch = super::Reserved::new(&self.pool.working, size);
                let descriptors = chunk.entries.iter().map(|e| e.descriptor).collect();
                let blobs = encoder(bytes, descriptors).await?;
                if blobs.len() != chunk.entries.len() {
                    return Err("Raster worker returned the wrong tile count".into());
                }
                for (entry, blob) in chunk.entries.iter().zip(blobs) {
                    if blob.descriptor != entry.descriptor {
                        return Err("Raster worker changed the pixel representation".into());
                    }
                    entry.tile.publish(Ok(blob))?;
                }
            }
            Ok(())
        }
        .await;
        if let Err(message) = &result {
            self.fail(message);
        }
        self.chunks.clear();
        result
    }
}

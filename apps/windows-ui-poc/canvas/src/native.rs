use crate::gpu::{Gpu, frequency, qpc};
use crate::cache::{Cache, ThumbnailReadback};
use crossbeam_channel::{Receiver, Sender, bounded};
use poc_common::{AppOptions, GlassRect, Load, PenSample};
use serde_json::{Value, json};
use std::{ffi::{CStr, c_char, c_void}, fs::File, io::{BufWriter, Write}, path::PathBuf, sync::{Arc, OnceLock, Mutex, atomic::{AtomicU8,Ordering}}, thread::JoinHandle, time::{Duration, Instant}};
use windows::{core::Interface, Win32::{Foundation::FILETIME, Graphics::DirectComposition::*, System::{Com::{CoInitializeEx, COINIT_MULTITHREADED}, Threading::{GetCurrentThread, GetThreadTimes}}}};

pub struct SharedGpu { pub device: wgpu::Device, pub queue: wgpu::Queue }
struct WorkerChannels { rx:Receiver<Command>, shared:Arc<OnceLock<SharedGpu>>, thumb_tx:Sender<ThumbnailReadback>, trace_path:Arc<Mutex<PathBuf>>, status:Arc<AtomicU8> }
enum Target { Visual(usize), Panel(usize) }
enum Command {
    Pen(Vec<PenSample>, i64), Resize(u32,u32,f32), Load(Load), Glass(Vec<GlassRect>), Rows(u32,u32), Motion(bool), Thumbnails(bool), Shutdown,
}
pub struct Canvas {
    tx: Sender<Command>,
    worker: Option<JoinHandle<Result<(), String>>>,
    shared: Arc<OnceLock<SharedGpu>>,
    thumbnails: crossbeam_channel::Receiver<ThumbnailReadback>,
    trace_path:Arc<Mutex<PathBuf>>,
    status:Arc<AtomicU8>,
}
impl Canvas {
    fn start(target: Target, width: u32, height: u32, scale: f32, options: AppOptions) -> Self {
        let (tx, rx) = bounded(65536);
        let (thumb_tx, thumbnails) = bounded(2);
        let trace_path=Arc::new(Mutex::new(options.trace.clone()));let worker_trace=trace_path.clone();
        let status=Arc::new(AtomicU8::new(0));let worker_status=status.clone();
        let shared = Arc::new(OnceLock::new()); let worker_shared = shared.clone();
        let worker = std::thread::Builder::new().name("poc-canvas".into()).spawn(move || {
            let result = run(target, width, height, scale, options, WorkerChannels {rx, shared:worker_shared, thumb_tx,trace_path:worker_trace,status:worker_status.clone()});
            worker_status.store(if result.is_ok(){2}else{3},Ordering::Release);
            result
        }).expect("canvas thread");
        Self { tx, worker: Some(worker), shared, thumbnails,trace_path,status }
    }
    pub fn new_for_visual(visual: IDCompositionVisual, width: u32, height: u32, scale: f32, options: AppOptions) -> Self { Self::start(Target::Visual(visual.into_raw() as usize),width,height,scale,options) }
    pub fn device(&self) -> Option<&SharedGpu> { self.shared.get() }
    fn send(&self, command: Command) -> Result<(), String> { self.tx.try_send(command).map_err(|e| format!("canvas command queue: {e}")) }
    pub fn pen(&self, samples: &[PenSample], receipt_qpc: i64) -> Result<(), String> { self.send(Command::Pen(samples.to_vec(), receipt_qpc)) }
    pub fn resize(&self, width:u32,height:u32,scale:f32) -> Result<(),String> { self.send(Command::Resize(width,height,scale)) }
    pub fn set_motion(&self, motion: bool) -> Result<(), String> { self.send(Command::Motion(motion)) }
    pub fn shutdown(&self) -> Result<(), String> { self.send(Command::Shutdown) }
    pub fn finish(mut self) -> Result<(),String> { self.worker.take().unwrap().join().map_err(|_| "canvas thread panicked".to_string())? }
}
fn thread_cpu_ms() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(),FILETIME::default(),FILETIME::default(),FILETIME::default());
    if unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) }.is_err() { return 0.0; }
    let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    (ticks(kernel)+ticks(user)) as f64 / 10000.0
}
fn flush_trace(path: &PathBuf, records: &[Value]) -> Result<(),String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let mut out = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
    for record in records { serde_json::to_writer(&mut out, record).map_err(|e| e.to_string())?; writeln!(out).map_err(|e| e.to_string())?; }
    out.flush().map_err(|e| e.to_string())
}
fn run(target: Target, width:u32,height:u32,scale:f32,options:AppOptions,channels:WorkerChannels) -> Result<(),String> {
    let trace_path=channels.trace_path.clone();
    let mut records = vec![json!({"kind":"startup","milestone":"canvas_thread","qpc":qpc(),"frequency":frequency()})];
    let result = render(target,[width,height],scale,&options,channels,&mut records);
    if let Err(error) = &result { records.push(json!({"kind":"error","qpc":qpc(),"error":error})); }
    let path=trace_path.lock().map_err(|_|"trace path poisoned")?.clone();
    flush_trace(&path,&records)?;
    result
}
fn drain_commands(rx:&Receiver<Command>,pending:&mut Vec<Command>)->bool {
    loop { match rx.try_recv() {Ok(Command::Shutdown)=>return true,Ok(command)=>pending.push(command),Err(crossbeam_channel::TryRecvError::Empty)=>return false,Err(crossbeam_channel::TryRecvError::Disconnected)=>return true} }
}
fn record_gpu_timings(gpu:&mut Gpu,records:&mut Vec<Value>)->Result<(),String> {
    if let Some(timestamps)=&mut gpu.timestamps {
        for timing in timestamps.poll(&gpu.device,&gpu.queue)? {records.push(json!({"kind":"gpu_timing","scope":"viewport","frame":timing.frame,"qpc":timing.qpc,"start_ticks":timing.start_ticks,"end_ticks":timing.end_ticks,"gpu_ms":timing.gpu_ms}));}
    }
    Ok(())
}
fn render(target: Target,dimensions:[u32;2],scale:f32,options:&AppOptions,channels:WorkerChannels,records:&mut Vec<Value>) -> Result<(),String> {
    let [width,height]=dimensions;
    let WorkerChannels {rx,shared,thumb_tx,status,..}=channels;
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| e.to_string())?; }
    let instance = Gpu::instance();
    let _panel_handle;
    let surface = match target {
        Target::Visual(raw) => {
            let visual = unsafe { IDCompositionVisual::from_raw(raw as *mut c_void) };
            unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CompositionVisual(visual.as_raw())) }.map_err(|e| e.to_string())?
        }
        Target::Panel(handle) => {
            use std::os::windows::io::FromRawHandle;
            _panel_handle=unsafe{std::os::windows::io::OwnedHandle::from_raw_handle(handle as *mut c_void)};
            unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::SurfaceHandle(handle as *mut c_void)) }.map_err(|e| e.to_string())?
        },
    };
    let mut gpu = Gpu::new(instance,surface,width,height,scale)?;
    let _ = shared.set(SharedGpu { device:gpu.device.clone(),queue:gpu.queue.clone() });
    status.store(1,Ordering::Release);
    let swapchain = unsafe { gpu.surface.as_hal::<wgpu::hal::api::Dx12>() }.and_then(|s|s.swap_chain()).map(|s|format!("0x{:x}",s.as_raw() as usize));
    records.push(json!({"swapchain":swapchain,"kind":"adapter","qpc":qpc(),"name":gpu.info.name,"device_type":format!("{:?}",gpu.info.device_type),"warp":gpu.info.device_type==wgpu::DeviceType::Cpu,"present_mode":format!("{:?}",gpu.config.present_mode),"maximum_frame_latency":1,"gpu_timestamps_supported":gpu.timestamps.is_some()}));
    let mut cache = Cache::new(&gpu.device,gpu.config.width,gpu.config.height);
    let mut glass = Vec::new();
    let mut thumbnails=options.thumbnails;
    let mut rows = (0,0); let mut thumbnail_at = Instant::now();
    let mut count = 0u32; let mut frame = 0u32; let mut motion = false; let mut load = options.load;
    let mut pending = Vec::new(); let mut dirty = true; let start = Instant::now(); let mut cpu_at = start;
    loop {
        if cpu_at.elapsed()>=Duration::from_secs(1) { records.push(json!({"kind":"thread_cpu","thread":"canvas","qpc":qpc(),"cpu_ms":thread_cpu_ms()})); cpu_at=Instant::now(); }
        let mut closing=false;
        if !dirty && !motion {
            match rx.recv_timeout(if thumbnails {Duration::from_millis(33).saturating_sub(thumbnail_at.elapsed())} else {Duration::from_secs(1)}) {
                Ok(Command::Shutdown) => closing=true,
                Ok(command) => pending.push(command),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {},
                Err(_) => closing=true,
            }
        }
        closing|=drain_commands(&rx,&mut pending);
        if closing && pending.is_empty() {break;}
        if !dirty && !motion && pending.is_empty() && !thumbnails {
            continue;
        }
        let frame_start = qpc(); let wait_start = qpc();
        let needs_frame = dirty || motion || pending.iter().any(|c|matches!(c,Command::Pen(..)|Command::Resize(..)|Command::Glass(..)|Command::Motion(..)));
        if needs_frame { gpu.wait()?; }
        let wait_end = qpc();
        closing|=drain_commands(&rx,&mut pending);
        let mut dabs = Vec::new();
        for command in pending.drain(..) {
            match command {
                Command::Pen(samples,receipt) => {
                    for sample in samples {
                        count = count.checked_add(1).ok_or("pen sample counter overflow")?;
                        if count > poc_common::PEN_MASK { return Err("pen sample marker would wrap".into()); }
                        records.push(json!({"kind":"input_receipt","qpc":receipt,"index":count-1,"timestamp":sample.timestamp}));
                        records.push(json!({"kind":"pen_consumed","qpc":qpc(),"index":count-1,"pen_samples":count,"sample":sample}));
                        if sample.pressure > 0.0 && sample.flags & 0x4 != 0 && sample.flags & 0x8000 == 0 { dabs.push(sample); }
                    }
                    dirty = true;
                }
                Command::Resize(w,h,s) => { gpu.resize(w,h,s); cache=Cache::new(&gpu.device,w.max(1),h.max(1)); dirty = true; }
                Command::Load(value) => load=value,
                Command::Motion(value) => { motion=value && load==Load::Slow; dirty|=motion; },
                Command::Thumbnails(value) => thumbnails=value,
                Command::Glass(rects) => {
                    let changed=glass.len()!=rects.len() || glass.iter().zip(&rects).any(|(a,b):(&GlassRect,&GlassRect)| [a.x,a.y,a.width,a.height]!=[b.x,b.y,b.width,b.height]);
                    if changed {records.push(json!({"kind":"glass","qpc":qpc(),"rects":rects}));dirty=true;}
                    glass=rects;
                },
                Command::Rows(first,n) => { rows=(first,n.min(crate::cache::THUMBNAIL_CAPACITY)); records.push(json!({"kind":"thumbnail_rows","qpc":qpc(),"first":first,"count":n})); },
                Command::Shutdown => unreachable!(),
            }
        }
        if !dirty && !motion {
            if closing {break;}
            if thumbnails && thumbnail_at.elapsed()>=Duration::from_millis(33) {
                let mut encoder=gpu.device.create_command_encoder(&Default::default());
                cache.tick_thumbnails(&gpu.device,&gpu.queue,&mut encoder,rows.0,rows.1,start.elapsed().as_secs_f32());
                gpu.queue.submit([encoder.finish()]);thumbnail_at=Instant::now();
            }
            if let Some(readback)=cache.poll_readback(&gpu.device)? { let _=thumb_tx.try_send(readback); }
            continue;
        }
        let seconds = start.elapsed().as_secs_f32();
        let (target,mut encoder) = gpu.encode(&dabs,count,frame)?;
        let viewport_view=gpu.viewport_view.clone();
        if cache.generation.load(std::sync::atomic::Ordering::Acquire)==0 {
            let generation=cache.encode_blur(&gpu.device,&gpu.queue,&mut encoder,&viewport_view);
            gpu.queue.submit([encoder.finish()]);cache.publish_blur(generation);
            encoder=gpu.device.create_command_encoder(&Default::default());
        }
        encoder.copy_texture_to_texture(gpu.viewport.as_image_copy(),target.texture.as_image_copy(),gpu.viewport.size());
        cache.encode_glass(&gpu.device,&gpu.queue,&mut encoder,&target.texture.create_view(&Default::default()),&glass,gpu.scale);
        if let Some(timestamps)=&mut gpu.timestamps && !timestamps.resolve(&mut encoder,frame as u64,frame_start as u64) {records.push(json!({"kind":"gpu_timestamp_missing","frame":frame}));}
        if load == Load::Slow { std::thread::sleep(Duration::from_millis(30)); }
        let submit_qpc = qpc(); let submission = gpu.queue.submit_for_present([encoder.finish()]);
        let present_qpc = qpc(); gpu.queue.present(target); gpu.in_flight=Some(submission);
        let mut blur_encoder=gpu.device.create_command_encoder(&Default::default());
        if thumbnails && thumbnail_at.elapsed()>=Duration::from_millis(33) {
            cache.tick_thumbnails(&gpu.device,&gpu.queue,&mut blur_encoder,rows.0,rows.1,seconds);thumbnail_at=Instant::now();
        }
        let generation=cache.encode_blur(&gpu.device,&gpu.queue,&mut blur_encoder,&viewport_view);
        gpu.in_flight=Some(gpu.queue.submit([blur_encoder.finish()]));cache.publish_blur(generation);
        record_gpu_timings(&mut gpu,records)?;
        if let Some(readback)=cache.poll_readback(&gpu.device)? {let _=thumb_tx.try_send(readback);}
        records.push(json!({"kind":"canvas_frame","qpc":frame_start,"end_qpc":qpc(),"wait_start":wait_start,"wait_end":wait_end,"submit_qpc":submit_qpc,"present_qpc":present_qpc,"frame":frame,"pen_samples":count}));
        if frame==0 { records.push(json!({"kind":"startup","milestone":"first_canvas_present","qpc":present_qpc})); }
        frame=frame.wrapping_add(1); dirty=false;
        if closing {break;}
    }
    gpu.wait()?;
    record_gpu_timings(&mut gpu,records)?;
    records.push(json!({"kind":"thread_cpu","thread":"canvas","qpc":qpc(),"cpu_ms":thread_cpu_ms()}));
    Ok(())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_create_for_panel(width:u32,height:u32,scale:f32,path:*const c_char,out_handle:*mut *mut c_void) -> *mut Canvas {
    if !scale.is_finite() || scale<=0.0 || width==0 || height==0 || path.is_null() || out_handle.is_null() { return std::ptr::null_mut(); }
    let Ok(handle) = (unsafe { DCompositionCreateSurfaceHandle(3,None) }) else { return std::ptr::null_mut(); };
    unsafe { *out_handle=handle.0; }
    let options=AppOptions { trace:PathBuf::from(unsafe{CStr::from_ptr(path)}.to_string_lossy().as_ref()),load:Load::Light,thumbnails:false,rows:1000 };
    Box::into_raw(Box::new(Canvas::start(Target::Panel(handle.0 as usize),width,height,scale,options)))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_resize(canvas:*mut Canvas,width:u32,height:u32,scale:f32)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.resize(width,height,scale).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_pen(canvas:*mut Canvas,samples:*const PenSample,count:usize,receipt:i64)->bool {
    if samples.is_null() || count>65536 { return false; }
    unsafe{canvas.as_ref()}.is_some_and(|c|c.pen(unsafe{std::slice::from_raw_parts(samples,count)},receipt).is_ok())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_glass(canvas:*mut Canvas,rects:*const GlassRect,count:usize)->bool {
    if (rects.is_null() && count!=0) || count>64 { return false; }
    let rects=if count==0 {Vec::new()} else {unsafe{std::slice::from_raw_parts(rects,count)}.to_vec()};
    unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Glass(rects)).is_ok())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_thumbnail_rows(canvas:*mut Canvas,first:u32,count:u32)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Rows(first,count)).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_load(canvas:*mut Canvas,slow:bool)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Load(if slow{Load::Slow}else{Load::Light})).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_motion(canvas:*mut Canvas,motion:bool)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.set_motion(motion).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_destroy(canvas:*mut Canvas) {
    if !canvas.is_null() { let canvas=unsafe{Box::from_raw(canvas)}; let _=canvas.shutdown(); }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_thumbnails_readback(canvas:*mut Canvas,first:*mut u32,count:*mut u32,out:*mut u8,capacity:usize)->bool {
    if first.is_null() || count.is_null() || out.is_null() {return false;}
    let Some(canvas)=(unsafe{canvas.as_ref()}) else{return false;};
    let Some(latest)=canvas.thumbnails.try_iter().last() else{return false;};
    if latest.rgba.len()>capacity {return false;}
    unsafe{*first=latest.first;*count=latest.count;std::ptr::copy_nonoverlapping(latest.rgba.as_ptr(),out,latest.rgba.len());}true
}

#[unsafe(no_mangle)]
pub extern "C" fn poc_spec_json()->*const c_char {
    static SPEC:OnceLock<std::ffi::CString>=OnceLock::new();
    SPEC.get_or_init(||std::ffi::CString::new(poc_common::specification().to_string()).expect("JSON contains no NUL")).as_ptr()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_spec_label(index:u32,out:*mut c_char,capacity:usize)->usize {
    let label=poc_common::label(index as usize);let needed=label.len()+1;
    if !out.is_null() && capacity>=needed {unsafe{std::ptr::copy_nonoverlapping(label.as_ptr(),out.cast(),label.len());*out.add(label.len())=0;}}
    needed
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_thumbnails(canvas:*mut Canvas,on:bool)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Thumbnails(on)).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_shutdown(canvas:*mut Canvas)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.shutdown().is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_status(canvas:*mut Canvas)->u32 { unsafe{canvas.as_ref()}.map_or(3,|c|c.status.load(Ordering::Acquire) as u32) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_trace_open(canvas:*mut Canvas,path:*const c_char)->bool {
    if path.is_null(){return false;}
    let Some(canvas)=(unsafe{canvas.as_ref()}) else{return false;};
    let Ok(mut current)=canvas.trace_path.try_lock() else{return false;};
    *current=PathBuf::from(unsafe{CStr::from_ptr(path)}.to_string_lossy().as_ref());true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_retains_pen_commands_accepted_before_the_shutdown_marker() {
        let (tx,rx)=bounded(4);
        tx.send(Command::Pen(vec![PenSample {timestamp:17,..Default::default()}],23)).unwrap();
        tx.send(Command::Shutdown).unwrap();
        let mut pending=Vec::new();
        assert!(drain_commands(&rx,&mut pending));
        assert!(matches!(&pending[..],[Command::Pen(samples,23)] if samples.len()==1 && samples[0].timestamp==17));
    }

    #[test]
    fn sender_disconnect_retains_every_previously_accepted_command() {
        let (tx,rx)=bounded(4);
        tx.send(Command::Pen(vec![PenSample {timestamp:17,..Default::default()}],23)).unwrap();
        tx.send(Command::Resize(800,600,1.5)).unwrap();
        drop(tx);
        let mut pending=Vec::new();
        assert!(drain_commands(&rx,&mut pending));
        assert!(matches!(&pending[..],[Command::Pen(samples,23),Command::Resize(800,600,1.5)] if samples[0].timestamp==17));
    }
}

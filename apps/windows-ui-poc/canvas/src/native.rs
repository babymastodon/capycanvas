use crate::gpu::{Gpu, frequency, qpc};
use crate::cache::{Cache,GlassPaint,ThumbnailReadback};
use poc_common::glass_motion::{GlassAnimation,GlassMotion,valid_rect};
use crossbeam_channel::{Receiver, Sender, bounded};
use poc_common::{AppOptions, GlassRect, Load, PenSample};
use serde_json::{Value, json};
use std::{ffi::{CStr, c_char, c_void}, fs::File, io::{BufWriter, Write}, path::PathBuf, sync::{Arc, OnceLock, Mutex, atomic::{AtomicBool,AtomicU8,Ordering}}, thread::JoinHandle, time::{Duration, Instant}};
use windows::{core::Interface, Win32::{Foundation::FILETIME, Graphics::DirectComposition::*, System::{Com::{CoInitializeEx, COINIT_MULTITHREADED}, Threading::{GetCurrentThread, GetThreadTimes}}}};

pub struct SharedGpu { pub device: wgpu::Device, pub queue: wgpu::Queue }
type ThumbnailSlot=Arc<Mutex<Option<ThumbnailReadback>>>;
type ReadyFunction=extern "C" fn(*mut c_void,bool);
#[derive(Clone)]
struct ReadyNotify {callback:ReadyFunction,context:usize,completed:Arc<AtomicBool>}
impl ReadyNotify {fn notify(&self,ready:bool){if !self.completed.swap(true,Ordering::AcqRel){(self.callback)(self.context as *mut c_void,ready);}}}
struct WorkerChannels { rx:Receiver<Command>, shared:Arc<OnceLock<SharedGpu>>, thumbnails:ThumbnailSlot, trace_path:Arc<Mutex<PathBuf>>, status:Arc<AtomicU8>, ready:Option<ReadyNotify>,presented:Option<ReadyNotify> }
enum Target { Visual(usize), Panel(usize) }
enum Command {
    Pen(Vec<PenSample>, i64), Resize(u32,u32,f32), Load(Load), Glass(Vec<GlassRect>),AnimateGlass(GlassAnimation),Rows(Vec<u32>), Motion(bool), Thumbnails(bool), Trace(String), PanelReady, Shutdown,
}
pub struct Canvas {
    tx: Sender<Command>,
    worker: Option<JoinHandle<Result<(), String>>>,
    shared: Arc<OnceLock<SharedGpu>>,
    thumbnails: ThumbnailSlot,
    trace_path:Arc<Mutex<PathBuf>>,
    status:Arc<AtomicU8>,
}
impl Canvas {
    fn start(target: Target, width: u32, height: u32, scale: f32, options: AppOptions,ready:Option<ReadyNotify>,presented:Option<ReadyNotify>) -> Self {
        let (tx, rx) = bounded(65536);
        let thumbnails=Arc::new(Mutex::new(None));let worker_thumbnails=thumbnails.clone();
        let trace_path=Arc::new(Mutex::new(options.trace.clone()));let worker_trace=trace_path.clone();
        let status=Arc::new(AtomicU8::new(0));let worker_status=status.clone();
        let shared = Arc::new(OnceLock::new()); let worker_shared = shared.clone();
        let worker = std::thread::Builder::new().name("poc-canvas".into()).spawn(move || {
            let result = run(target, width, height, scale, options, WorkerChannels {rx, shared:worker_shared, thumbnails:worker_thumbnails,trace_path:worker_trace,status:worker_status.clone(),ready:ready.clone(),presented:presented.clone()});
            worker_status.store(if result.is_ok(){2}else{3},Ordering::Release);
            if let Some(ready)=ready {ready.notify(false);}
            if let Some(presented)=presented {presented.notify(false);}
            result
        }).expect("canvas thread");
        Self { tx, worker: Some(worker), shared, thumbnails,trace_path,status }
    }
    pub fn new_for_visual(visual: IDCompositionVisual, width: u32, height: u32, scale: f32, options: AppOptions) -> Self { Self::start(Target::Visual(visual.into_raw() as usize),width,height,scale,options,None,None) }
    pub fn device(&self) -> Option<&SharedGpu> { self.shared.get() }
    fn send(&self, command: Command) -> Result<(), String> { self.tx.try_send(command).map_err(|e| format!("canvas command queue: {e}")) }
    pub fn pen(&self, samples: &[PenSample], receipt_qpc: i64) -> Result<(), String> { self.send(Command::Pen(samples.to_vec(), receipt_qpc)) }
    pub fn resize(&self, width:u32,height:u32,scale:f32) -> Result<(),String> { self.send(Command::Resize(width,height,scale)) }
    pub fn animate_glass(&self,animation:GlassAnimation)->Result<(),String> {animation.validate()?;self.send(Command::AnimateGlass(animation))}
    pub fn set_motion(&self, motion: bool) -> Result<(), String> { self.send(Command::Motion(motion)) }
    pub fn trace(&self,record:&Value)->Result<(),String> {self.send(Command::Trace(record.to_string()))}
    pub fn shutdown(&self) -> Result<(), String> { self.send(Command::Shutdown) }
    pub fn finish(mut self) -> Result<(),String> {let worker=self.worker.take().unwrap();drop(self.tx);worker.join().map_err(|_|"canvas thread panicked".to_string())?}
}
fn publish_thumbnail(slot:&ThumbnailSlot,readback:ThumbnailReadback)->Result<(),String> {
    *slot.lock().map_err(|_|"thumbnail slot poisoned")?=Some(readback);Ok(())
}
fn take_thumbnail(slot:&ThumbnailSlot,row_capacity:u32,byte_capacity:usize)->Option<ThumbnailReadback> {
    let mut current=slot.try_lock().ok()?;
    if current.as_ref().is_none_or(|ready|ready.row_ids.len()>row_capacity as usize || ready.rgba.len()>byte_capacity) {return None;}
    current.take()
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
    let WorkerChannels {rx,shared,thumbnails:thumbnail_slot,status,ready,mut presented,..}=channels;
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| e.to_string())?; }
    let instance = Gpu::instance();
    let _panel_handle;
    let panel=matches!(&target,Target::Panel(_));
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
    if panel {gpu.set_panel_transform()?;}
    let _ = shared.set(SharedGpu { device:gpu.device.clone(),queue:gpu.queue.clone() });
    status.store(1,Ordering::Release);
    if let Some(ready)=ready {ready.notify(true);}
    let swapchain = unsafe { gpu.surface.as_hal::<wgpu::hal::api::Dx12>() }.and_then(|s|s.swap_chain()).map(|s|format!("0x{:x}",s.as_raw() as usize));
    records.push(json!({"swapchain":swapchain,"kind":"adapter","qpc":qpc(),"name":gpu.info.name,"device_type":format!("{:?}",gpu.info.device_type),"warp":gpu.info.device_type==wgpu::DeviceType::Cpu,"present_mode":format!("{:?}",gpu.config.present_mode),"maximum_frame_latency":1,"gpu_timestamps_supported":gpu.timestamps.is_some()}));
    let mut cache = Cache::new(&gpu.device,gpu.config.width,gpu.config.height);
    let mut glass = GlassMotion::default();
    let mut thumbnails=options.thumbnails;
    let mut rows = Vec::new();let mut thumbnail_dirty=false;
    let thumbnail_period=Duration::from_secs_f64(1.0/poc_common::THUMBNAIL_HZ as f64);
    let mut thumbnail_at=Instant::now();let mut thumbnail_poll_at=Instant::now();
    let mut count = 0u32; let mut frame = 0u32; let mut motion = false; let mut load = options.load;
    let mut pending = Vec::new(); let mut dirty = true; let mut panel_ready=!panel;let start = Instant::now(); let mut cpu_at = start;
    loop {
        if cpu_at.elapsed()>=Duration::from_secs(1) { records.push(json!({"kind":"thread_cpu","thread":"canvas","qpc":qpc(),"cpu_ms":thread_cpu_ms()})); cpu_at=Instant::now(); }
        let mut closing=false;
        let driving=motion && load==Load::Slow || glass.needs_frame();
        if !panel_ready || !dirty && !driving {
            match rx.recv_timeout(if thumbnails && !rows.is_empty() {thumbnail_period.saturating_sub(thumbnail_at.elapsed())}else if cache.has_pending_readback() {thumbnail_period.saturating_sub(thumbnail_poll_at.elapsed())}else{Duration::from_secs(1)}) {
                Ok(Command::Shutdown) => closing=true,
                Ok(command) => pending.push(command),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {},
                Err(_) => closing=true,
            }
        }
        closing|=drain_commands(&rx,&mut pending);
        panel_ready|=pending.iter().any(|command|matches!(command,Command::PanelReady));
        if !panel_ready && !closing {continue;}
        if closing && pending.is_empty() {break;}
        let frame_start = qpc(); let wait_start = qpc();
        let needs_frame = panel_ready && (dirty || driving || pending.iter().any(|c|matches!(c,Command::Pen(..)|Command::Resize(..)|Command::Glass(..)|Command::AnimateGlass(..)|Command::Motion(..)|Command::Load(..))));
        if needs_frame { gpu.wait()?; }
        let wait_end = qpc();
        closing|=drain_commands(&rx,&mut pending);
        let mut dabs = Vec::new();
        for command in pending.drain(..) {
            match command {
                Command::Pen(samples,receipt) => {
                    records.push(json!({"kind":"pen_history","qpc":receipt,"samples":samples.len()}));
                    for sample in samples {
                        count = count.checked_add(1).ok_or("pen sample counter overflow")?;
                        if count > poc_common::PEN_MASK { return Err("pen sample marker would wrap".into()); }
                        records.push(json!({"kind":"input_receipt","qpc":receipt,"index":count-1,"timestamp":sample.timestamp}));
                        records.push(json!({"kind":"pen_consumed","qpc":qpc(),"index":count-1,"pen_samples":count,"sample":sample}));
                        if sample.pressure > 0.0 && sample.flags & 0x4 != 0 && sample.flags & 0x8000 == 0 { dabs.push(sample); }
                    }
                    dirty = true;
                }
                Command::Resize(w,h,s) => { gpu.resize(w,h,s);if panel {gpu.set_panel_transform()?;}cache=Cache::new(&gpu.device,w.max(1),h.max(1));thumbnail_dirty= !rows.is_empty();dirty=true; }
                Command::Load(value) => load=value,
                Command::Motion(value) => {motion=value;dirty|=motion && load==Load::Slow;},
                Command::Thumbnails(value)=>{thumbnail_dirty|=thumbnails!=value;thumbnails=value;},
                Command::Glass(rects)=>{
                    let changed=glass.publish(rects.clone())?;
                    if changed {records.push(json!({"kind":"glass","qpc":qpc(),"rects":rects}));dirty=true;}
                },
                Command::AnimateGlass(animation)=>{
                    let accepted=glass.animate(animation,frequency())?;
                    records.push(json!({"kind":"glass_animation","qpc":qpc(),"animation":accepted}));dirty=true;
                },
                Command::Rows(row_ids)=>{if rows!=row_ids {records.push(json!({"kind":"thumbnail_rows","qpc":qpc(),"row_ids":row_ids}));thumbnail_dirty=true;rows=row_ids;}},
                Command::Trace(raw)=>{
                    let record:Value=serde_json::from_str(&raw).map_err(|e|format!("native trace: {e}"))?;
                    if !record.is_object() {return Err("native trace must be a JSON object".into());}
                    records.push(record);
                },
                Command::PanelReady=>panel_ready=true,
                Command::Shutdown => unreachable!(),
            }
        }
        if !panel_ready {break;}
        let driving=motion && load==Load::Slow || glass.needs_frame();
        dirty|=glass.advance(qpc(),frequency());
        if !dirty && !driving {
            if closing {break;}
            let ticked=thumbnail_dirty || thumbnails && !rows.is_empty() && thumbnail_at.elapsed()>=thumbnail_period;
            if ticked {
                let mut encoder=gpu.device.create_command_encoder(&Default::default());
                cache.tick_thumbnails(&gpu.device,&gpu.queue,&mut encoder,&rows,start.elapsed().as_secs_f32(),thumbnails);
                gpu.queue.submit([encoder.finish()]);thumbnail_at=Instant::now();thumbnail_dirty=false;
            }
            if cache.has_pending_readback() && (ticked || thumbnail_poll_at.elapsed()>=thumbnail_period) {
                if let Some(readback)=cache.poll_readback(&gpu.device,&gpu.queue)? {publish_thumbnail(&thumbnail_slot,readback)?;}
                thumbnail_poll_at=Instant::now();
            }
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
        cache.encode_glass(&gpu.device,&gpu.queue,&mut encoder,&target.texture.create_view(&Default::default()),glass.rects(),GlassPaint {scale:gpu.scale,opacity:glass.opacity()});
        if let Some(timestamps)=&mut gpu.timestamps && !timestamps.resolve(&mut encoder,frame as u64,frame_start as u64) {records.push(json!({"kind":"gpu_timestamp_missing","frame":frame}));}
        if load == Load::Slow { std::thread::sleep(Duration::from_millis(30)); }
        let submit_qpc = qpc(); let submission = gpu.queue.submit_for_present([encoder.finish()]);
        let present_qpc = qpc(); gpu.present(target); gpu.in_flight=Some(submission);
        let mut blur_encoder=gpu.device.create_command_encoder(&Default::default());
        let ticked=thumbnail_dirty || thumbnails && !rows.is_empty() && thumbnail_at.elapsed()>=thumbnail_period;
        if ticked {
            cache.tick_thumbnails(&gpu.device,&gpu.queue,&mut blur_encoder,&rows,seconds,thumbnails);thumbnail_at=Instant::now();thumbnail_dirty=false;
        }
        let generation=cache.encode_blur(&gpu.device,&gpu.queue,&mut blur_encoder,&viewport_view);
        gpu.in_flight=Some(gpu.queue.submit([blur_encoder.finish()]));cache.publish_blur(generation);
        if let Some(presented)=presented.take(){gpu.wait()?;presented.notify(true);}
        record_gpu_timings(&mut gpu,records)?;
        if cache.has_pending_readback() && (ticked || thumbnail_poll_at.elapsed()>=thumbnail_period) {
            if let Some(readback)=cache.poll_readback(&gpu.device,&gpu.queue)? {publish_thumbnail(&thumbnail_slot,readback)?;}
            thumbnail_poll_at=Instant::now();
        }
        records.push(json!({"kind":"canvas_frame","qpc":frame_start,"end_qpc":qpc(),"wait_start":wait_start,"wait_end":wait_end,"submit_qpc":submit_qpc,"present_qpc":present_qpc,"frame":frame,"pen_samples":count}));
        if frame==0 { records.push(json!({"kind":"startup","milestone":"first_canvas_present","qpc":present_qpc})); }
        frame=frame.wrapping_add(1); dirty=false;
        if closing {break;}
    }
    if frame>0 {gpu.wait()?;}
    record_gpu_timings(&mut gpu,records)?;
    records.push(json!({"kind":"thread_cpu","thread":"canvas","qpc":qpc(),"cpu_ms":thread_cpu_ms()}));
    Ok(())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_create_for_panel(width:u32,height:u32,scale:f32,path:*const c_char,out_handle:*mut *mut c_void,ready:Option<ReadyFunction>,presented:Option<ReadyFunction>,context:*mut c_void) -> *mut Canvas {
    if !scale.is_finite() || scale<=0.0 || width==0 || height==0 || path.is_null() || out_handle.is_null() { return std::ptr::null_mut(); }
    let Ok(handle) = (unsafe { DCompositionCreateSurfaceHandle(3,None) }) else { return std::ptr::null_mut(); };
    unsafe { *out_handle=handle.0; }
    let options=AppOptions { trace:PathBuf::from(unsafe{CStr::from_ptr(path)}.to_string_lossy().as_ref()),load:Load::Light,thumbnails:false,rows:1000,ready_event:None,ui_stall_ms:0,ui_stall_every_ms:100 };
    let ready=ready.map(|callback|ReadyNotify {callback,context:context as usize,completed:Arc::new(AtomicBool::new(false))});
    let presented=presented.map(|callback|ReadyNotify {callback,context:context as usize,completed:Arc::new(AtomicBool::new(false))});
    Box::into_raw(Box::new(Canvas::start(Target::Panel(handle.0 as usize),width,height,scale,options,ready,presented)))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_resize(canvas:*mut Canvas,width:u32,height:u32,scale:f32)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.resize(width,height,scale).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_start_panel(canvas:*mut Canvas)->bool {unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::PanelReady).is_ok())}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_pen(canvas:*mut Canvas,samples:*const PenSample,count:usize,receipt:i64)->bool {
    if samples.is_null() || count>65536 { return false; }
    unsafe{canvas.as_ref()}.is_some_and(|c|c.pen(unsafe{std::slice::from_raw_parts(samples,count)},receipt).is_ok())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_glass(canvas:*mut Canvas,rects:*const GlassRect,count:usize)->bool {
    if (rects.is_null() && count!=0) || count>64 { return false; }
    let rects=if count==0 {Vec::new()} else {unsafe{std::slice::from_raw_parts(rects,count)}.to_vec()};
    if !rects.iter().copied().all(valid_rect) {return false;}
    unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Glass(rects)).is_ok())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_thumbnail_rows(canvas:*mut Canvas,row_ids:*const u32,count:u32)->bool {
    if count>crate::cache::THUMBNAIL_CAPACITY || row_ids.is_null() && count!=0 {return false;}
    let rows=if count==0 {Vec::new()}else{unsafe{std::slice::from_raw_parts(row_ids,count as usize)}.to_vec()};
    if rows.iter().any(|row|*row>65_535) {return false;}
    unsafe{canvas.as_ref()}.is_some_and(|canvas|canvas.send(Command::Rows(rows)).is_ok())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_load(canvas:*mut Canvas,slow:bool)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.send(Command::Load(if slow{Load::Slow}else{Load::Light})).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_set_motion(canvas:*mut Canvas,motion:bool)->bool { unsafe{canvas.as_ref()}.is_some_and(|c|c.set_motion(motion).is_ok()) }
#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_destroy(canvas:*mut Canvas)->bool {
    if canvas.is_null() {return false;}
    let canvas=*unsafe{Box::from_raw(canvas)};let _=canvas.shutdown();canvas.finish().is_ok()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_thumbnails_readback(canvas:*mut Canvas,row_ids:*mut u32,row_capacity:u32,count:*mut u32,out:*mut u8,capacity:usize)->bool {
    if row_ids.is_null() || count.is_null() || out.is_null() {return false;}
    let Some(canvas)=(unsafe{canvas.as_ref()}) else{return false;};
    let Some(latest)=take_thumbnail(&canvas.thumbnails,row_capacity,capacity) else{return false;};
    unsafe{*count=latest.row_ids.len() as u32;std::ptr::copy_nonoverlapping(latest.row_ids.as_ptr(),row_ids,latest.row_ids.len());std::ptr::copy_nonoverlapping(latest.rgba.as_ptr(),out,latest.rgba.len());}true
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

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_trace_record(canvas:*mut Canvas,record:*const c_char,length:usize)->bool {
    if record.is_null() || length>1_048_576 {return false;}
    let Ok(raw)=std::str::from_utf8(unsafe{std::slice::from_raw_parts(record.cast(),length)}) else{return false;};
    unsafe{canvas.as_ref()}.is_some_and(|canvas|canvas.send(Command::Trace(raw.into())).is_ok())
}

#[unsafe(no_mangle)]
unsafe extern "C" fn poc_canvas_animate_glass(canvas:*mut Canvas,animation:*const GlassAnimation)->bool {
    let Some(animation)=(unsafe{animation.as_ref()}) else{return false;};
    unsafe{canvas.as_ref()}.is_some_and(|canvas|canvas.animate_glass(*animation).is_ok())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_completion_notifies_the_host_once_before_terminal_cleanup() {
        use std::sync::atomic::AtomicUsize;
        extern "C" fn record(context:*mut c_void,ready:bool){unsafe{&*(context.cast::<AtomicUsize>())}.fetch_add(256+usize::from(ready),Ordering::Relaxed);}
        for success in [false,true] {
            let seen=AtomicUsize::new(0);
            let ready=ReadyNotify {callback:record,context:(&seen as *const AtomicUsize) as usize,completed:Arc::new(AtomicBool::new(false))};
            ready.clone().notify(success);ready.notify(false);
            assert_eq!(seen.load(Ordering::Relaxed),256+usize::from(success));
        }
    }
    #[test]
    fn finish_disconnects_a_full_command_queue_before_joining() {
        let (tx,rx)=bounded(1);tx.send(Command::Rows(vec![70])).unwrap();
        let mut canvas=Canvas {tx,worker:None,shared:Arc::new(OnceLock::new()),thumbnails:Arc::new(Mutex::new(None)),trace_path:Arc::new(Mutex::new(PathBuf::new())),status:Arc::new(AtomicU8::new(0))};
        assert!(canvas.shutdown().is_err());
        canvas.worker=Some(std::thread::spawn(move|| {
            if !matches!(rx.recv(),Ok(Command::Rows(rows)) if rows==[70]) {return Err("accepted row command was lost".into());}
            if !matches!(rx.recv_timeout(Duration::from_secs(1)),Err(crossbeam_channel::RecvTimeoutError::Disconnected)) {return Err("sender remained alive during worker join".into());}
            Ok(())
        }));
        canvas.finish().unwrap();
    }
    #[test]
    fn delayed_static_thumbnail_consumer_receives_latest_ids_and_keeps_insufficient_capacity_pending() {
        let slot=Arc::new(Mutex::new(None));
        for row in [0,70,140] {publish_thumbnail(&slot,ThumbnailReadback {row_ids:vec![row],rgba:vec![row as u8,0,0,255]}).unwrap();}
        assert!(take_thumbnail(&slot,0,4).is_none());
        let latest=take_thumbnail(&slot,1,4).unwrap();assert_eq!(latest.row_ids,[140]);assert_eq!(latest.rgba,[140,0,0,255]);assert!(take_thumbnail(&slot,1,4).is_none());
    }
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

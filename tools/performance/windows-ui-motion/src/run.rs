use std::{fs::{self,File},io::{BufWriter,Write},path::Path,sync::{Arc,atomic::{AtomicBool,Ordering},mpsc},time::Duration};
use serde::{Deserialize,Serialize};
use windows::Win32::{Foundation::*,System::Performance::*,UI::{HiDpi::*,WindowsAndMessaging::*}};
pub type Result<T>=std::result::Result<T,Box<dyn std::error::Error+Send+Sync>>;
#[derive(Clone,Deserialize,Serialize)]
pub struct Plan {pub process_id:u32,pub hwnd:usize,pub workload:String,pub seconds:f64,pub scale:f64,pub refresh_hz:f64,pub point:[i32;2],pub amplitude:f64,pub axis:usize,pub window:[i32;4],#[serde(default)]pub menu:[i32;2],pub probes:Vec<crate::probes::Spec>,#[serde(default)]pub no_capture:bool,#[serde(default)]pub profile:bool}
pub fn qpc()->i64 {let mut value=0;unsafe{QueryPerformanceCounter(&mut value).expect("QPC");}value}
pub fn frequency()->i64 {static VALUE:std::sync::OnceLock<i64>=std::sync::OnceLock::new();*VALUE.get_or_init(||{let mut v=0;unsafe{QueryPerformanceFrequency(&mut v).expect("QPC frequency");}v})}
pub fn write_line(file:&mut BufWriter<File>,value:&serde_json::Value)->Result<()> {serde_json::to_writer(&mut *file,value)?;file.write_all(b"\n")?;Ok(())}
pub fn main()->Result<()> {
    let args:Vec<_>=std::env::args().skip(1).collect();if args.len()==3&&args[0]=="--monitor"{return crate::monitor::run(Path::new(&args[1]),Path::new(&args[2]));}
    if args.len()!=2{return Err("usage: windows-ui-motion [--monitor] PLAN.json NEW_OUTPUT_DIRECTORY".into());}
    let plan:Plan=serde_json::from_reader(File::open(&args[0])?)?;let out=Path::new(&args[1]);fs::create_dir(out)?;
    fs::write(out.join("plan.json"),serde_json::to_vec_pretty(&plan)?)?;
    if !plan.seconds.is_finite()||!(0.1..=10.).contains(&plan.seconds)||plan.axis>1||plan.probes.is_empty()||plan.probes.iter().any(|s|s.rect[2]<=0||s.rect[3]<=0||(s.kind!="black-grid"&&s.rect[2] as i64*s.rect[3] as i64>262144)){return Err("invalid bounded workload plan".into());}
    if plan.probes.iter().any(|s|s.axis>1||(s.kind=="edge"&&s.anchor+32>s.rect[2+s.axis] as usize)){return Err("edge template is outside its ROI".into());}
    unsafe{SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;}
    let hwnd=HWND(plan.hwnd as _);let mut owner=0;let tid=unsafe{GetWindowThreadProcessId(hwnd,Some(&mut owner))};
    if owner!=plan.process_id||tid==0||unsafe{GetForegroundWindow()}!=hwnd{return Err("plan does not name the owned foreground drawing".into());}
    let (_,_,scale)=crate::launch::bounds(hwnd)?;let refresh=crate::launch::refresh(hwnd)?;
    fs::write(out.join("display.json"),serde_json::to_vec_pretty(&crate::launch::display_mode(hwnd,refresh,scale)?)?)?;
    if (scale-plan.scale).abs()>0.001||(refresh-plan.refresh_hz).abs()>0.1||refresh<119.{return Err(format!("desktop scale/refresh differs from preflight: scale={scale}, expected={}, refresh={refresh}, expected={}",plan.scale,plan.refresh_hz).into());}
    let stop=Arc::new(AtomicBool::new(false));let (tx,rx)=mpsc::channel();
    let capture=if plan.no_capture{None}else{let path=out.join("frames.jsonl");let specs=plan.probes.clone();let done=stop.clone();Some(std::thread::spawn(move||crate::capture::run(&path,specs,done,tx)))};
    let result=(||->Result<()>{
        let adapter=if plan.no_capture{serde_json::Value::Null}else{rx.recv_timeout(Duration::from_secs(20)).map_err(|_|"DD readiness deadline")?.map_err(|e|format!("DD init: {e}"))?};
        let sampler=crate::cpu::ForeignCpu::new(plan.process_id,tid)?;
        let mut samples=Vec::new();std::thread::sleep(Duration::from_millis(500));let initial=sampler.sample()?;let idle_start=initial["qpc"].as_i64().ok_or("CPU sample has no timestamp")?;samples.push(initial);
        for _ in 0..10{std::thread::sleep(Duration::from_millis(100));samples.push(sampler.sample()?);}
        let marker=if plan.profile{Some(crate::profile::Marker::new()?)}else{None};
        let profile_start=marker.as_ref().map(|m|m.write("begin")).transpose()?;
        let start=qpc();samples.push(sampler.sample()?);let owned=plan.clone();let path=out.join("inputs.jsonl");
        let injection=std::thread::spawn(move||crate::workloads::run(&owned,start,&path));
        while !injection.is_finished(){std::thread::sleep(Duration::from_millis(100));samples.push(sampler.sample()?);}
        let injection_result=injection.join().map_err(|_|"injection panicked")?;let end=qpc();let profile_end=marker.as_ref().map(|m|m.write("end")).transpose()?;samples.push(sampler.sample()?);injection_result?;
        fs::write(out.join("profile-markers.json"),serde_json::to_vec(&serde_json::json!({"begin":profile_start,"end":profile_end}))?)?;
        std::thread::sleep(Duration::from_millis(400));
        fs::write(out.join("cpu.json"),serde_json::to_vec(&samples)?)?;
        fs::write(out.join("meta.json"),serde_json::to_vec_pretty(&serde_json::json!({"process_id":plan.process_id,"ui_thread_id":tid,"workload":plan.workload,"nominal_hz":crate::workloads::nominal_hz(&plan),"qpc_frequency":frequency(),"idle_start_qpc":idle_start,"motion_start_qpc":start,"motion_end_qpc":start+(plan.seconds*frequency() as f64) as i64,"observer_end_qpc":end,"refresh_hz":refresh,"scale":scale,"adapter":adapter,"no_capture":plan.no_capture}))?)?;Ok(())
    })();
    stop.store(true,Ordering::Release);let capture_result=match capture{Some(t)=>t.join().map_err(|_|"capture panicked")?,None=>Ok(())};
    match (&result,&capture_result){(Ok(_),Ok(_))=>fs::write(out.join("completed.json"),b"{\"ok\":true}")?,_=>fs::write(out.join("failure.txt"),format!("workload={result:?}\ncapture={capture_result:?}\n"))?,}
    result?;capture_result?;Ok(())
}

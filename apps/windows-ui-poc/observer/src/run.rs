use crate::{capture,launch,workloads};
use std::{fs::{self,File},io::{BufReader,BufRead,BufWriter,Write},path::PathBuf,sync::{Arc,atomic::{AtomicBool,AtomicUsize,AtomicI32,AtomicU32,Ordering},mpsc},time::{Duration,Instant}};
use windows::Win32::{Foundation::*,System::{Performance::*,Threading::*},UI::HiDpi::*};

pub type Result<T> = std::result::Result<T,Box<dyn std::error::Error+Send+Sync>>;
#[derive(Clone)]
pub struct Options {pub arm:String,pub exe:PathBuf,pub workload:String,pub contacts:u32,pub seconds:f64,pub warmup:f64,pub profile:String,pub out:PathBuf,pub no_capture:bool,pub samples:Option<u32>,pub app_args:Vec<String>}
impl Options {
    fn parse()->Result<Self> {
        let mut args=std::env::args().skip(1);
        let command=args.next().ok_or("expected run or pen-check")?;
        poc_observer::diagnostic::sample_target(&command,None)?;
        let diagnostic=command=="pen-check";let mut requested_samples=None;
        let mut result=Self {arm:String::new(),exe:PathBuf::new(),workload:if diagnostic{"paint-still".into()}else{String::new()},contacts:if diagnostic{1}else{3},seconds:if diagnostic{poc_observer::diagnostic::MAX_SECONDS}else{6.0},warmup:if diagnostic{0.0}else{1.0},profile:"normal".into(),out:PathBuf::new(),no_capture:false,samples:None,app_args:Vec::new()};
        while let Some(arg)=args.next(){
            if arg=="--"{result.app_args.extend(args);break;}
            if arg=="--no-capture"{result.no_capture=true;continue;}
            if diagnostic&&matches!(arg.as_str(),"--workload"|"--contacts"|"--seconds"|"--warmup"){return Err(format!("{arg} is a run option; pen-check owns its count and 60 second deadline").into());}
            let value=args.next().ok_or_else(||format!("missing value for {arg}"))?;
            match arg.as_str(){"--arm"=>result.arm=value,"--exe"=>result.exe=value.into(),"--workload"=>result.workload=value,"--contacts"=>result.contacts=value.parse()?,"--seconds"=>result.seconds=value.parse()?,"--warmup"=>result.warmup=value.parse()?,"--profile"=>result.profile=value,"--samples"=>requested_samples=Some(value.parse()?),"--out"=>result.out=value.into(),_=>return Err(format!("unknown observer option: {arg}").into())}
        }
        result.samples=poc_observer::diagnostic::sample_target(&command,requested_samples)?;
        if !matches!(result.arm.as_str(),"retained"|"winui")||!matches!(result.profile.as_str(),"normal"|"lowcpu")||result.exe.as_os_str().is_empty()||result.out.as_os_str().is_empty()||result.workload.is_empty(){return Err("arm, exe, workload and out are required; profile is normal or lowcpu".into());}
        if result.contacts==0||result.contacts>100||!result.seconds.is_finite()||!(0.1..=60.0).contains(&result.seconds)||!result.warmup.is_finite()||!(0.0..=10.0).contains(&result.warmup){return Err("invalid contact count or duration".into());}
        result.exe=poc_observer::paths::plain(&fs::canonicalize(&result.exe)?.to_string_lossy()).into();Ok(result)
    }
}
pub fn qpc()->i64 {let mut value=0;unsafe{QueryPerformanceCounter(&mut value).expect("QPC unavailable");}value}
pub fn frequency()->i64 {static FREQ:std::sync::OnceLock<i64>=std::sync::OnceLock::new();*FREQ.get_or_init(||{let mut value=0;unsafe{QueryPerformanceFrequency(&mut value).expect("QPC frequency unavailable");}value})}
pub fn write_line(file:&mut BufWriter<File>,value:&serde_json::Value)->Result<()> {serde_json::to_writer(&mut *file,value)?;file.write_all(b"\n")?;Ok(())}
fn write_json(path:impl AsRef<std::path::Path>,value:&impl serde::Serialize)->Result<()> {let file=File::create(path)?;serde_json::to_writer_pretty(file,value)?;Ok(())}
pub fn main()->Result<()> {
    let options=Options::parse()?;
    unsafe {SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;}
    fs::create_dir_all(&options.out)?;
    for contact in 0..options.contacts {run_contact(&options,contact)?;}
    Ok(())
}
fn run_contact(options:&Options,contact:u32)->Result<()> {
    let out=options.out.join(format!("contact-{contact:03}"));fs::create_dir(&out)?;
    let out:PathBuf=poc_observer::paths::plain(&fs::canonicalize(out)?.to_string_lossy()).into();
    let mut observer_cpu=poc_observer::cpu::ThreadCpu::new("main",&out.join("observer-cpu.jsonl"),None)?;
    let stop=Arc::new(AtomicBool::new(false));let window=Arc::new(AtomicUsize::new(0));let slider_y=Arc::new(AtomicI32::new(-1));
    let ready=poc_common::ready::ReadyEvent::create()?;let pen_seen=Arc::new(AtomicU32::new(0));
    let (capture_thread,adapter)=if options.no_capture {(None,serde_json::Value::Null)}else{
        let (tx,rx)=mpsc::channel();let path=out.join("frames.jsonl");let hwnd=window.clone();let done=stop.clone();let probe=slider_y.clone();let observed=pen_seen.clone();
        let thread=std::thread::spawn(move||capture::run(&path,hwnd,done,tx,probe,observed));
        let adapter=match rx.recv_timeout(Duration::from_secs(30)){Ok(Ok(info))=>info,other=>{stop.store(true,Ordering::Release);let _=thread.join();return Err(format!("Desktop Duplication initialization failed: {other:?}").into());}};
        (Some(thread),adapter)
    };
    let mut args=options.app_args.clone();for i in 1..args.len(){if args[i-1]=="--trace"{args[i]=poc_observer::paths::plain(&args[i]);}}args.extend(["--trace".into(),out.join("canvas.jsonl").to_string_lossy().into_owned(),"--ready-event".into(),ready.name().into()]);
    if !args.iter().any(|s|s=="--thumbnails"){args.extend(["--thumbnails".into(),if matches!(options.workload.as_str(),"scroll-wheel"|"scroll-touch"|"stall-scroll"){"on".into()}else{"off".into()}]);}
    let process=match launch::Process::start(&options.exe,&args,options.profile=="lowcpu"){Ok(p)=>p,Err(e)=>{stop.store(true,Ordering::Release);if let Some(t)=capture_thread{let _=t.join();}return Err(e);}};
    let process_handle=process.info.hProcess.0 as usize;let sample_path=out.join("process.jsonl");let sample_stop=stop.clone();
    let sampler=std::thread::spawn(move||->Result<()>{let mut cpu=poc_observer::cpu::ThreadCpu::new("process_sampler",&sample_path.with_file_name("observer-cpu.jsonl"),None)?;let mut writer=BufWriter::new(File::create(sample_path)?);while !sample_stop.load(Ordering::Acquire){cpu.sample()?;launch::sample_process(HANDLE(process_handle as _),&mut writer)?;std::thread::sleep(Duration::from_millis(100));}cpu.finish()?;writer.flush()?;Ok(())});
    let result=(||->Result<()> {
        let hwnd=process.window()?;window.store(hwnd.0 as usize,Ordering::Release);launch::position(hwnd)?;
        let window_qpc=qpc();ready.wait(process.info.hProcess,60_000)?;let input_ready_qpc=qpc();let (bounds,origin,scale)=launch::bounds(hwnd)?;let refresh=launch::refresh()?;
        let targets=workloads::Targets::resolve(hwnd,&options.workload)?;
        if options.workload=="scrub-while-painting"{slider_y.store((targets.slider.top+targets.slider.bottom)/2-origin.y,Ordering::Release);}
        let calibration=workloads::calibration(options,&targets,bounds,origin,scale);
        let start=qpc();let duration=if options.workload=="startup"{((process.launch_qpc as f64/frequency() as f64)+15.0-start as f64/frequency() as f64).max(0.0)}else{options.seconds+options.warmup};
        let end=start+(duration*frequency() as f64).round() as i64;
        let mut meta=serde_json::json!({"arm":options.arm,"workload":options.workload,"profile":options.profile,"contact":contact,"qpc_frequency":frequency(),"logical_cpus":unsafe{GetActiveProcessorCount(0xffff)},"launch_qpc":process.launch_qpc,"window_qpc":window_qpc,"input_ready_qpc":input_ready_qpc,"motion_start_qpc":if options.workload=="startup"{None}else{Some(start+(options.warmup*frequency() as f64).round() as i64)},"motion_end_qpc":if options.workload=="startup"{None}else{Some(end)},"idle_start_qpc":if options.workload=="startup"{Some(process.launch_qpc+5*frequency())}else{None},"idle_end_qpc":if options.workload=="startup"{Some(process.launch_qpc+15*frequency())}else{None},"scale":scale,"refresh_hz":refresh,"width":bounds.right,"height":bounds.bottom,"client_origin":[origin.x,origin.y],"no_capture":options.no_capture,"cpu_rate":process.cpu_rate,"process_id":process.info.dwProcessId,"hwnd":hwnd.0 as usize,"executable":options.exe,"app_args":args,"capture_adapter":adapter,"calibration_start_qpc":calibration.map(|_|start+(0.25*options.warmup*frequency() as f64).round() as i64),"calibration_end_qpc":calibration.map(|_|start+(options.warmup*frequency() as f64).round() as i64),"calibration_expected_value":calibration,"requested_pen_samples":options.samples,"diagnostic_deadline_qpc":options.samples.map(|_|end)});
        meta["display_mode"]=launch::display_mode(hwnd,refresh,scale)?;
        meta["probe_geometry"]=serde_json::json!({"glass":"right-gutter","gutter_px":poc_common::GLASS_GUTTER_PX});
        write_json(out.join("meta.json"),&meta)?;
        let injector=if options.workload=="startup"{File::create(out.join("inputs.jsonl"))?;None}else{let clone=options.clone();let raw=hwnd.0 as usize;let pid=process.info.dwProcessId;let path=out.join("inputs.jsonl");let done=stop.clone();Some(std::thread::spawn(move||workloads::run(&clone,(raw,pid),targets,start,&path,done,contact)))};
        let mut exited=false;
        while qpc()<end {if options.samples.is_some()&&injector.as_ref().is_some_and(std::thread::JoinHandle::is_finished){break;}observer_cpu.sample()?;if unsafe{WaitForSingleObject(process.info.hProcess,50)}==WAIT_OBJECT_0{stop.store(true,Ordering::Release);exited=true;break;}}
        let injection_result=if let Some(thread)=injector{thread.join().map_err(|_|"injection thread panicked")?}else{Ok(())};
        if options.samples.is_some(){
            let summary:serde_json::Value=serde_json::from_reader(File::open(out.join("injection.json"))?)?;
            meta["motion_start_qpc"]=summary["active_start_qpc"].clone();meta["motion_end_qpc"]=summary["active_end_qpc"].clone();meta["active_seconds"]=summary["measured_seconds"].clone();
            write_json(out.join("meta.json"),&meta)?;
        }
        injection_result?;
        if exited{return Err("application exited during workload".into());}
        if options.workload!="startup"{let settle=Instant::now()+Duration::from_millis(350);while Instant::now()<settle{std::thread::sleep(Duration::from_millis(10));}}
        if !options.no_capture && matches!(options.workload.as_str(),"paint-still"|"scrub-while-painting") {
            let expected=read_jsonl(out.join("inputs.jsonl"))?.iter().filter(|input|input["kind"]=="pen").count() as u32;
            let deadline=qpc()+10*frequency();
            loop {
                if unsafe{WaitForSingleObject(process.info.hProcess,0)}==WAIT_OBJECT_0{return Err("application exited before terminal pen display".into());}
                if capture_thread.as_ref().is_some_and(std::thread::JoinHandle::is_finished){return Err("capture ended before terminal pen display".into());}
                if !poc_observer::diagnostic::terminal_pen_pending(pen_seen.load(Ordering::Acquire),expected,qpc(),deadline)?{break;}
                observer_cpu.sample()?;unsafe{WaitForSingleObject(process.info.hProcess,50)};
            }
        }
        Ok(())
    })();
    stop.store(true,Ordering::Release);
    let sampling=match sampler.join(){Ok(result)=>result,Err(_)=>Err("process sampler panicked".into())};
    let capture_result=match capture_thread {
        Some(thread)=>match thread.join(){Ok(Ok(stats))=>Ok(Some(stats)),Ok(Err(e))=>Err(e),Err(_)=>Err("capture thread panicked".into())},
        None=>Ok(None),
    };
    let close=process.close(HWND(window.load(Ordering::Acquire) as _));
    let closed_gracefully=close.is_ok();
    let mut failure=None;let mut failures=Vec::new();
    preserve_failure(&mut failure,&mut failures,"workload",result);
    preserve_failure(&mut failure,&mut failures,"process_sampling",sampling);
    let stats=match capture_result{Ok(stats)=>stats,Err(error)=>{preserve_failure(&mut failure,&mut failures,"capture",Err(error));None}};
    preserve_failure(&mut failure,&mut failures,"graceful_close",close);
    if let Some(stats)=&stats{preserve_failure(&mut failure,&mut failures,"capture_evidence",write_json(out.join("capture.json"),stats));}
    if failure.is_none(){
        let gates=(||->Result<()> {
            if let Some(stats)=&stats && (stats.capture_failure.is_some()||stats.valid_ui_markers==0||stats.valid_canvas_markers==0){return Err(format!("capture gate failed: {}",serde_json::to_string(stats)?).into());}
            let trace=read_jsonl(out.join("canvas.jsonl"))?;
            poc_observer::gates::canvas_trace(&trace)?;
            if !options.no_capture{
                let frames=read_jsonl(out.join("frames.jsonl"))?;
                let startup=poc_observer::gates::startup(&frames,&trace,process.launch_qpc)?;
                write_json(out.join("startup.json"),&startup)?;
            }
            if matches!(options.workload.as_str(),"paint-still"|"scrub-while-painting"){
                let inputs=read_jsonl(out.join("inputs.jsonl"))?;
                let expected=inputs.iter().filter(|input|input["kind"]=="pen").count() as u32;
                let mut evidence=serde_json::to_value(poc_observer::gates::pen_delivery(&inputs,&trace,expected)?)?;
                evidence["history"]=poc_observer::diagnostic::history(&trace);
                write_json(out.join("pen-delivery.json"),&evidence)?;
                if let Some(stats)=&stats && stats.max_pen_samples!=expected{return Err(format!("pen display gate failed: expected {expected}, observed {}",stats.max_pen_samples).into());}
            }
            Ok(())
        })();
        preserve_failure(&mut failure,&mut failures,"data_gates",gates);
    }
    observer_cpu.finish()?;
    if let Some(error)=failure{
        if let Err(write_error)=write_json(out.join("failures.json"),&serde_json::json!({"failures":failures,"closed_gracefully":closed_gracefully})){eprintln!("could not write observer failure evidence: {write_error}");}
        return Err(error);
    }
    write_json(out.join("completed.json"),&serde_json::json!({"completed":true,"capture_enabled":!options.no_capture,"closed_gracefully":closed_gracefully}))?;
    Ok(())
}
fn preserve_failure(primary:&mut Option<Box<dyn std::error::Error+Send+Sync>>,failures:&mut Vec<serde_json::Value>,stage:&str,result:Result<()>){
    if let Err(error)=result{failures.push(serde_json::json!({"stage":stage,"message":error.to_string()}));if primary.is_none(){*primary=Some(error);}}
}
fn read_jsonl(path:PathBuf)->Result<Vec<serde_json::Value>>{
    BufReader::new(File::open(path)?).lines().map(|line|Ok(serde_json::from_str(&line?)?)).collect()
}

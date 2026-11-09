use crate::run::{Result, qpc, write_line};
use std::{io::BufWriter, mem::size_of, path::Path, time::{Duration, Instant}};
use windows::{core::{PCWSTR,PWSTR}, Win32::{Foundation::*, Graphics::{Gdi::*,Dwm::*}, System::{Threading::*,JobObjects::*,ProcessStatus::*}, UI::{HiDpi::*,WindowsAndMessaging::*}}};

pub struct Process { pub info: PROCESS_INFORMATION, job: HANDLE, pub launch_qpc: i64, pub cpu_rate: Option<u32> }
impl Process {
    pub fn start(exe: &Path, args: &[String], lowcpu: bool) -> Result<Self> {
        let image=poc_observer::paths::plain(&exe.to_string_lossy());
        let directory=poc_observer::paths::plain(&exe.parent().ok_or("application image has no absolute parent directory")?.to_string_lossy());
        let directory:Vec<u16>=directory.encode_utf16().chain(Some(0)).collect();
        let path: Vec<u16> = image.encode_utf16().chain(Some(0)).collect();
        let command = std::iter::once(image).chain(args.iter().cloned()).map(|s| quote(&s)).collect::<Vec<_>>().join(" ");
        let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        let mut info = PROCESS_INFORMATION::default();
        let startup = STARTUPINFOW { cb:size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let launch_qpc = qpc();
        unsafe { CreateProcessW(PCWSTR(path.as_ptr()),Some(PWSTR(command.as_mut_ptr())),None,None,false,CREATE_SUSPENDED,None,PCWSTR(directory.as_ptr()),&startup,&mut info)?; }
        let mut result = Self { info, job:HANDLE::default(), launch_qpc, cpu_rate:None };
        if lowcpu {
            let cpus = unsafe { GetActiveProcessorCount(0xffff) };
            if cpus < 4 { return Err("lowcpu requires at least four logical CPUs".into()); }
            result.job = unsafe { CreateJobObjectW(None,PCWSTR::null())? };
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION { BasicLimitInformation:JOBOBJECT_BASIC_LIMIT_INFORMATION { LimitFlags:JOB_OBJECT_LIMIT_AFFINITY, Affinity:15, ..Default::default() }, ..Default::default() };
            let rate = (0.5 * 4.0 / cpus as f64 * 10000.0).round().clamp(1.0,10000.0) as u32;
            let cpu = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION { ControlFlags:JOB_OBJECT_CPU_RATE_CONTROL_ENABLE|JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, Anonymous:JOBOBJECT_CPU_RATE_CONTROL_INFORMATION_0 { CpuRate:rate } };
            unsafe {
                SetInformationJobObject(result.job,JobObjectExtendedLimitInformation,&limits as *const _ as _,size_of_val(&limits) as u32)?;
                SetInformationJobObject(result.job,JobObjectCpuRateControlInformation,&cpu as *const _ as _,size_of_val(&cpu) as u32)?;
                AssignProcessToJobObject(result.job,info.hProcess)?;
            }
            result.cpu_rate=Some(rate);
        }
        if unsafe { ResumeThread(info.hThread) }==u32::MAX { return Err(windows::core::Error::from_thread().into()); }
        Ok(result)
    }
    pub fn window(&self) -> Result<HWND> {
        let deadline=Instant::now()+Duration::from_secs(60);
        while Instant::now()<deadline {
            let mut find=Find { pid:self.info.dwProcessId, window:HWND::default() };
            unsafe { let _=EnumWindows(Some(find_window),LPARAM(&mut find as *mut _ as isize)); }
            if !find.window.is_invalid() { return Ok(find.window); }
            if unsafe { WaitForSingleObject(self.info.hProcess,0) }==WAIT_OBJECT_0 { return Err("application exited before exposing its window".into()); }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err("application window deadline elapsed".into())
    }
    pub fn close(&self, mut hwnd:HWND) -> Result<()> {
        unsafe {
            if WaitForSingleObject(self.info.hProcess,0)==WAIT_OBJECT_0 {return self.exit_result();}
            let mut owner=0;
            if !hwnd.is_invalid(){GetWindowThreadProcessId(hwnd,Some(&mut owner));}
            if owner!=self.info.dwProcessId {
                let mut find=Find{pid:self.info.dwProcessId,window:HWND::default()};
                let _=EnumWindows(Some(find_window),LPARAM(&mut find as *mut _ as isize));hwnd=find.window;
                if hwnd.is_invalid(){return Err("running application has no owned window for graceful close".into());}
            }
            PostMessageW(Some(hwnd),WM_CLOSE,WPARAM(0),LPARAM(0))?;
            if WaitForSingleObject(self.info.hProcess,10000)!=WAIT_OBJECT_0 { return Err("application did not close within 10 seconds".into()); }
        }
        self.exit_result()
    }
    fn exit_result(&self)->Result<()> {
        let mut code=0;unsafe {GetExitCodeProcess(self.info.hProcess,&mut code)?;}
        poc_observer::gates::process_exit(code).map_err(Into::into)
    }
}
impl Drop for Process {
    fn drop(&mut self) { unsafe {
        if !self.info.hProcess.is_invalid() && WaitForSingleObject(self.info.hProcess,0)!=WAIT_OBJECT_0 { let _=TerminateProcess(self.info.hProcess,1); let _=WaitForSingleObject(self.info.hProcess,5000); }
        for h in [self.info.hThread,self.info.hProcess,self.job] { if !h.is_invalid() { let _=CloseHandle(h); } }
    } }
}
struct Find { pid:u32, window:HWND }
unsafe extern "system" fn find_window(hwnd:HWND, value:LPARAM) -> windows::core::BOOL {
    let find=unsafe { &mut *(value.0 as *mut Find) };
    let mut pid=0;
    unsafe { GetWindowThreadProcessId(hwnd,Some(&mut pid)); }
    if pid!=find.pid {return true.into();}
    let mut title=vec![0u16;unsafe{GetWindowTextLengthW(hwnd)}.max(0) as usize+1];
    let length=unsafe{GetWindowTextW(hwnd,&mut title)}.max(0) as usize;
    if poc_observer::placement::application_window(find.pid,pid,&String::from_utf16_lossy(&title[..length]),unsafe {IsWindowVisible(hwnd).as_bool()},unsafe {GetWindow(hwnd,GW_OWNER).map(|h|h.is_invalid()).unwrap_or(true)}) {find.window=hwnd;return false.into();}
    true.into()
}
pub fn bounds(hwnd:HWND)->Result<(RECT,POINT,f64)> {
    let mut rect=RECT::default(); let mut origin=POINT::default();
    unsafe { GetClientRect(hwnd,&mut rect)?; if !ClientToScreen(hwnd,&mut origin).as_bool() { return Err(windows::core::Error::from_thread().into()); } }
    Ok((rect,origin,unsafe { GetDpiForWindow(hwnd) } as f64/96.0))
}
pub fn position(hwnd:HWND)->Result<()> {
    let dpi=unsafe { GetDpiForWindow(hwnd) };
    let scale=dpi as f64/96.0;
    let (client,origin,_)=bounds(hwnd)?;
    let mut outer=RECT::default();
    unsafe {
        GetWindowRect(hwnd,&mut outer)?;
        let [x,y,width,height]=poc_observer::placement::client_at_origin([outer.left,outer.top,outer.right,outer.bottom],[client.right-client.left,client.bottom-client.top],[origin.x,origin.y],[(poc_common::CLIENT[0] as f64*scale).round() as i32,(poc_common::CLIENT[1] as f64*scale).round() as i32]);
        SetWindowPos(hwnd,None,x,y,width,height,SWP_NOZORDER|SWP_NOACTIVATE)?;
        if !SetForegroundWindow(hwnd).as_bool() { return Err("could not acquire owned window foreground".into()); }
    }
    let (actual,at,_)=bounds(hwnd)?;
    if at.x!=0||at.y!=0||actual.right!=(poc_common::CLIENT[0] as f64*scale).round() as i32||actual.bottom!=(poc_common::CLIENT[1] as f64*scale).round() as i32{return Err("actual client placement differs from the requested primary-display rectangle".into());}
    Ok(())
}
pub fn refresh()->Result<f64> {
    let mut info=DWM_TIMING_INFO { cbSize:size_of::<DWM_TIMING_INFO>() as u32,..Default::default() };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(),&mut info)?; }
    if info.rateRefresh.uiDenominator==0 { return Err("DWM returned no refresh denominator".into()); }
    Ok(info.rateRefresh.uiNumerator as f64/info.rateRefresh.uiDenominator as f64)
}
pub fn display_mode(hwnd:HWND,refresh:f64,scale:f64)->Result<serde_json::Value> {
    let monitor=unsafe {MonitorFromWindow(hwnd,MONITOR_DEFAULTTOPRIMARY)};
    let mut info=MONITORINFOEXW::default();info.monitorInfo.cbSize=size_of::<MONITORINFOEXW>() as u32;
    if !unsafe {GetMonitorInfoW(monitor,&mut info.monitorInfo)}.as_bool(){return Err(windows::core::Error::from_thread().into());}
    let rect=info.monitorInfo.rcMonitor;
    Ok(serde_json::json!({"monitor":monitor.0 as usize,"device":String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0'),"desktop_rect":[rect.left,rect.top,rect.right,rect.bottom],"width":rect.right-rect.left,"height":rect.bottom-rect.top,"refresh_hz":refresh,"scale":scale}))
}
pub fn sample_process(handle:HANDLE, file:&mut BufWriter<std::fs::File>)->Result<()> {
    let (mut create,mut exit,mut kernel,mut user)=(FILETIME::default(),FILETIME::default(),FILETIME::default(),FILETIME::default());
    let mut memory=PROCESS_MEMORY_COUNTERS_EX { cb:size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,..Default::default() };
    unsafe { GetProcessTimes(handle,&mut create,&mut exit,&mut kernel,&mut user)?; GetProcessMemoryInfo(handle,&mut memory as *mut _ as _,size_of_val(&memory) as u32)?; }
    let time=|v:FILETIME| (v.dwLowDateTime as u64)|((v.dwHighDateTime as u64)<<32);
    write_line(file,&serde_json::json!({"qpc":qpc(),"cpu_ms":(time(kernel)+time(user)) as f64/10000.0,"private_bytes":memory.PrivateUsage}))
}
fn quote(text:&str)->String {
    let mut out=String::from("\""); let mut slashes=0;
    for c in text.chars() {
        if c=='\\' { slashes+=1; } else { if c=='\"' { out.extend(std::iter::repeat_n('\\',slashes*2+1)); } else { out.extend(std::iter::repeat_n('\\',slashes)); } slashes=0; out.push(c); }
    }
    out.extend(std::iter::repeat_n('\\',slashes*2)); out.push('"'); out
}

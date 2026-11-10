use crate::run::{Result,qpc};
use std::mem::size_of;
use windows::Win32::{Foundation::*,Graphics::{Gdi::*,Dwm::*},System::{Threading::*,ProcessStatus::*},UI::{HiDpi::*,WindowsAndMessaging::*}};
pub fn bounds(hwnd:HWND)->Result<(RECT,POINT,f64)> {
    let mut rect=RECT::default(); let mut origin=POINT::default();
    unsafe { GetClientRect(hwnd,&mut rect)?; if !ClientToScreen(hwnd,&mut origin).as_bool() { return Err(windows::core::Error::from_thread().into()); } }
    Ok((rect,origin,unsafe { GetDpiForWindow(hwnd) } as f64/96.0))
}
pub fn compositor_refresh()->Result<f64> {
    let mut info=DWM_TIMING_INFO { cbSize:size_of::<DWM_TIMING_INFO>() as u32,..Default::default() };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(),&mut info)?; }
    if info.rateRefresh.uiDenominator==0 { return Err("DWM returned no refresh denominator".into()); }
    Ok(info.rateRefresh.uiNumerator as f64/info.rateRefresh.uiDenominator as f64)
}
pub fn refresh(hwnd:HWND)->Result<f64>{
    let monitor=unsafe{MonitorFromWindow(hwnd,MONITOR_DEFAULTTONULL)};
    let mut info=MONITORINFOEXW::default();info.monitorInfo.cbSize=size_of::<MONITORINFOEXW>() as u32;
    if !unsafe{GetMonitorInfoW(monitor,&mut info.monitorInfo)}.as_bool(){return Err(windows::core::Error::from_thread().into());}
    if info.monitorInfo.dwFlags&MONITORINFOF_PRIMARY==0{return Err("app is not on the primary GPU output".into());}
    let mut mode=DEVMODEW{dmSize:size_of::<DEVMODEW>() as u16,..Default::default()};
    if !unsafe{EnumDisplaySettingsW(windows::core::PCWSTR(info.szDevice.as_ptr()),ENUM_CURRENT_SETTINGS,&mut mode)}.as_bool(){return Err(windows::core::Error::from_thread().into());}
    Ok(mode.dmDisplayFrequency as f64)
}
pub fn display_mode(hwnd:HWND,refresh:f64,scale:f64)->Result<serde_json::Value> {
    let monitor=unsafe {MonitorFromWindow(hwnd,MONITOR_DEFAULTTOPRIMARY)};
    let mut info=MONITORINFOEXW::default();info.monitorInfo.cbSize=size_of::<MONITORINFOEXW>() as u32;
    if !unsafe {GetMonitorInfoW(monitor,&mut info.monitorInfo)}.as_bool(){return Err(windows::core::Error::from_thread().into());}
    let rect=info.monitorInfo.rcMonitor;
    Ok(serde_json::json!({"monitor":monitor.0 as usize,"device":String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0'),"desktop_rect":[rect.left,rect.top,rect.right,rect.bottom],"width":rect.right-rect.left,"height":rect.bottom-rect.top,"refresh_hz":refresh,"dwm_observed_refresh_hz":compositor_refresh().ok(),"scale":scale}))
}
pub fn sample_process(handle:HANDLE)->Result<serde_json::Value> {
    let (mut create,mut exit,mut kernel,mut user)=(FILETIME::default(),FILETIME::default(),FILETIME::default(),FILETIME::default());
    let mut memory=PROCESS_MEMORY_COUNTERS_EX { cb:size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,..Default::default() };
    unsafe { GetProcessTimes(handle,&mut create,&mut exit,&mut kernel,&mut user)?; GetProcessMemoryInfo(handle,&mut memory as *mut _ as _,size_of_val(&memory) as u32)?; }
    let time=|v:FILETIME| (v.dwLowDateTime as u64)|((v.dwHighDateTime as u64)<<32);
    Ok(serde_json::json!({"qpc":qpc(),"cpu_ms":(time(kernel)+time(user)) as f64/10000.0,"private_bytes":memory.PrivateUsage}))
}

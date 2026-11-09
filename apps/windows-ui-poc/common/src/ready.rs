use windows::{core::PCWSTR,Win32::{Foundation::{CloseHandle,HANDLE,WAIT_OBJECT_0,WAIT_TIMEOUT},System::{Performance::QueryPerformanceCounter,Threading::{CreateEventW,OpenEventW,SetEvent,WaitForMultipleObjects,EVENT_MODIFY_STATE}}}};

struct KernelHandle(HANDLE);
impl Drop for KernelHandle {fn drop(&mut self){unsafe{let _=CloseHandle(self.0);}}}
pub struct ReadyEvent {handle:KernelHandle,name:String}
impl ReadyEvent {
    pub fn create()->Result<Self,String> {
        use std::sync::atomic::{AtomicU64,Ordering};
        static SEQUENCE:AtomicU64=AtomicU64::new(0);
        let mut clock=0;unsafe{QueryPerformanceCounter(&mut clock)}.map_err(|error|error.to_string())?;
        let name=format!("Local\\CapyPocReady-{}-{clock}-{}",std::process::id(),SEQUENCE.fetch_add(1,Ordering::Relaxed));
        let wide:Vec<u16>=name.encode_utf16().chain(Some(0)).collect();
        let handle=unsafe{CreateEventW(None,true,false,PCWSTR(wide.as_ptr()))}.map_err(|error|error.to_string())?;
        Ok(Self {handle:KernelHandle(handle),name})
    }
    pub fn name(&self)->&str {&self.name}
    pub fn wait(&self,process:HANDLE,milliseconds:u32)->Result<(),String> {
        let result=unsafe{WaitForMultipleObjects(&[self.handle.0,process],false,milliseconds)};
        if result==WAIT_OBJECT_0 {Ok(())}else if result==WAIT_TIMEOUT {Err("application input-readiness deadline elapsed".into())}else if result.0==WAIT_OBJECT_0.0+1 {Err("application exited before input readiness".into())}else{Err(windows::core::Error::from_thread().to_string())}
    }
}
pub fn signal(name:&str)->Result<(),String> {
    let wide:Vec<u16>=name.encode_utf16().chain(Some(0)).collect();
    let handle=KernelHandle(unsafe{OpenEventW(EVENT_MODIFY_STATE,false,PCWSTR(wide.as_ptr()))}.map_err(|error|error.to_string())?);
    unsafe{SetEvent(handle.0)}.map_err(|error|error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::{OpenProcess,PROCESS_SYNCHRONIZE};
    #[test]
    fn an_existing_event_stays_unready_until_the_host_signals_it() {
        let first=ReadyEvent::create().unwrap();let second=ReadyEvent::create().unwrap();assert_ne!(first.name(),second.name());
        let process=KernelHandle(unsafe{OpenProcess(PROCESS_SYNCHRONIZE,false,std::process::id())}.unwrap());
        let timeout="application input-readiness deadline elapsed";
        assert_eq!(first.wait(process.0,0).unwrap_err(),timeout);assert_eq!(second.wait(process.0,0).unwrap_err(),timeout);
        signal(first.name()).unwrap();first.wait(process.0,0).unwrap();assert_eq!(second.wait(process.0,0).unwrap_err(),timeout);
        signal(second.name()).unwrap();second.wait(process.0,0).unwrap();
    }
}

use crate::run::{Result,qpc,frequency};
use std::mem::size_of;
use windows::Win32::{Foundation::*,System::Threading::*,UI::{Input::KeyboardAndMouse::*,WindowsAndMessaging::*}};

pub struct Injector { owner:u32,timer:HANDLE,mouse_down:bool }
impl Injector {
    pub fn new(owner:u32)->Result<Self> {
        let timer=unsafe {CreateWaitableTimerExW(None,windows::core::PCWSTR::null(),CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,TIMER_ALL_ACCESS.0)}.map_err(|error|format!("CreateWaitableTimerExW(HIGH_RESOLUTION): {error}"))?;
        Ok(Self {owner,timer,mouse_down:false})
    }
    pub fn wait(&self,at:i64)->Result<()> {
        let remaining=at-qpc();
        if remaining>0 {
            let due=-((remaining as i128*10_000_000+frequency() as i128-1)/frequency() as i128).max(1) as i64;
            unsafe {SetWaitableTimer(self.timer,&due,0,None,None,false)}.map_err(|error|format!("SetWaitableTimer(injection,target_qpc={at},relative_100ns={due}): {error}"))?;
            let timeout=(remaining as f64*1000.0/frequency() as f64).ceil() as u32+5000;
            let result=unsafe {WaitForSingleObject(self.timer,timeout)};
            if result!=WAIT_OBJECT_0 {return Err(format!("WaitForSingleObject(injection timer,target_qpc={at},timeout_ms={timeout}) returned {result:?}").into());}
        }
        Ok(())
    }
    fn guard(&self,point:Option<POINT>)->Result<()> {
        let mut owner=0;
        let thread=unsafe {GetWindowThreadProcessId(GetForegroundWindow(),Some(&mut owner))};
        if thread==0||owner!=self.owner {return Err("injection lost owned foreground window".into());}
        if let Some(point)=point {
            let mut target=0;let thread=unsafe {GetWindowThreadProcessId(WindowFromPoint(point),Some(&mut target))};
            if thread==0||target!=self.owner {return Err(format!("injection point ({},{}) is outside owned process",point.x,point.y).into());}
        }
        Ok(())
    }
    pub fn mouse(&mut self,point:POINT,button:Option<bool>,wheel:i32,check_point:bool)->Result<()> {
        self.guard(check_point.then_some(point))?;
        let (left,top,width,height)=unsafe {(GetSystemMetrics(SM_XVIRTUALSCREEN),GetSystemMetrics(SM_YVIRTUALSCREEN),GetSystemMetrics(SM_CXVIRTUALSCREEN),GetSystemMetrics(SM_CYVIRTUALSCREEN))};
        let mut flags=MOUSEEVENTF_MOVE|MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK;
        if let Some(down)=button {flags|=if down {MOUSEEVENTF_LEFTDOWN}else{MOUSEEVENTF_LEFTUP};}
        if wheel!=0 {flags|=MOUSEEVENTF_WHEEL;}
        let input=INPUT {r#type:INPUT_MOUSE,Anonymous:INPUT_0 {mi:MOUSEINPUT {dx:((point.x-left) as i64*65535/(width-1) as i64) as i32,dy:((point.y-top) as i64*65535/(height-1) as i64) as i32,mouseData:wheel as u32,dwFlags:flags,..Default::default()}}};
        if unsafe {SendInput(&[input],size_of::<INPUT>() as i32)}!=1 {return Err(format!("SendInput(mouse,point=({},{}),button={button:?},wheel={wheel}): {}",point.x,point.y,windows::core::Error::from_thread()).into());}
        if let Some(down)=button {self.mouse_down=down;}
        Ok(())
    }
    pub fn key(&self,character:u16)->Result<()> {
        self.guard(None)?;
        let input=|up|INPUT {r#type:INPUT_KEYBOARD,Anonymous:INPUT_0 {ki:KEYBDINPUT {wScan:character,dwFlags:KEYEVENTF_UNICODE|if up {KEYEVENTF_KEYUP}else{KEYBD_EVENT_FLAGS(0)},..Default::default()}}};
        if unsafe {SendInput(&[input(false),input(true)],size_of::<INPUT>() as i32)}!=2 {return Err(format!("SendInput(KEYEVENTF_UNICODE,character=0x{character:x}): {}",windows::core::Error::from_thread()).into());}
        Ok(())
    }
    pub fn virtual_key(&self,key:u16,up:bool)->Result<()> {
        self.guard(None)?;
        let input=INPUT {r#type:INPUT_KEYBOARD,Anonymous:INPUT_0 {ki:KEYBDINPUT {wVk:VIRTUAL_KEY(key),dwFlags:if up{KEYEVENTF_KEYUP}else{KEYBD_EVENT_FLAGS(0)},..Default::default()}}};
        if unsafe{SendInput(&[input],size_of::<INPUT>() as i32)}!=1{return Err("virtual key injection failed".into());}Ok(())
    }
    pub fn check_window(&self,hwnd:HWND)->Result<()> {
        self.guard(None)?;
        let mut owner=0;let thread=unsafe{GetWindowThreadProcessId(hwnd,Some(&mut owner))};
        if owner!=self.owner||thread==0{return Err("resize lost the owned drawing window".into());}Ok(())
    }
}
impl Drop for Injector {
    fn drop(&mut self) {unsafe {
        if self.mouse_down {let input=INPUT {r#type:INPUT_MOUSE,Anonymous:INPUT_0 {mi:MOUSEINPUT {dwFlags:MOUSEEVENTF_LEFTUP,..Default::default()}}};let _=SendInput(&[input],size_of::<INPUT>() as i32);}
        let _=CloseHandle(self.timer);
    }}
}

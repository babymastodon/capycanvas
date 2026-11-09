use crate::run::{Result,qpc,frequency};
use std::mem::size_of;
use windows::Win32::{Foundation::*,System::Threading::*,UI::{Controls::*,Input::{Pointer::*,KeyboardAndMouse::*},WindowsAndMessaging::*}};

pub struct Injector { owner:u32,pen:Option<HSYNTHETICPOINTERDEVICE>,touch:Option<HSYNTHETICPOINTERDEVICE>,timer:HANDLE,pen_down:bool,touch_down:bool,mouse_down:bool,last_pen:POINT,last_touch:POINT,last_pen_stamp:i64,last_touch_stamp:i64 }
impl Injector {
    pub fn new(owner:u32)->Result<Self> {
        let timer=unsafe {CreateWaitableTimerExW(None,windows::core::PCWSTR::null(),CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,TIMER_ALL_ACCESS.0)}.map_err(|error|format!("CreateWaitableTimerExW(HIGH_RESOLUTION): {error}"))?;
        Ok(Self {owner,pen:None,touch:None,timer,pen_down:false,touch_down:false,mouse_down:false,last_pen:POINT::default(),last_touch:POINT::default(),last_pen_stamp:0,last_touch_stamp:0})
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
        unsafe {GetWindowThreadProcessId(GetForegroundWindow(),Some(&mut owner));}
        if owner!=self.owner {return Err("injection lost owned foreground window".into());}
        if let Some(point)=point {
            unsafe {GetWindowThreadProcessId(WindowFromPoint(point),Some(&mut owner));}
            if owner!=self.owner {return Err(format!("injection point ({},{}) is outside owned process",point.x,point.y).into());}
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
    pub fn mouse_active(&self)->bool { self.mouse_down }
    pub fn pointer_active(&self,pen:bool)->bool { if pen {self.pen_down}else{self.touch_down} }
    pub fn last_pointer(&self,pen:bool)->POINT { if pen {self.last_pen}else{self.last_touch} }
    pub fn prepare_pointer(&mut self,pen:bool)->Result<()> {
        let slot=if pen {&mut self.pen}else{&mut self.touch};
        if slot.is_none() {*slot=Some(unsafe {CreateSyntheticPointerDevice(if pen {PT_PEN}else{PT_TOUCH},1,POINTER_FEEDBACK_NONE)}.map_err(|error|format!("CreateSyntheticPointerDevice(pen={pen}): {error}"))?);}
        Ok(())
    }
    pub fn pointer(&mut self,pen:bool,point:POINT,phase:u32,axes:[f64;4],stamp:i64)->Result<()> {
        self.guard(Some(point))?;
        self.prepare_pointer(pen)?;
        let device=if pen {self.pen.unwrap()}else{self.touch.unwrap()};
        let flags=if phase==0 {POINTER_FLAG_DOWN|POINTER_FLAG_INRANGE|POINTER_FLAG_INCONTACT}else if phase==2 {POINTER_FLAG_UP}else{POINTER_FLAG_UPDATE|POINTER_FLAG_INRANGE|POINTER_FLAG_INCONTACT};
        let info=POINTER_INFO {pointerType:if pen {PT_PEN}else{PT_TOUCH},pointerId:0,pointerFlags:flags,ptPixelLocation:point,PerformanceCount:stamp as u64,..Default::default()};
        let value=if pen {POINTER_TYPE_INFO {r#type:PT_PEN,Anonymous:POINTER_TYPE_INFO_0 {penInfo:POINTER_PEN_INFO {pointerInfo:info,penMask:PEN_MASK_PRESSURE|PEN_MASK_ROTATION|PEN_MASK_TILT_X|PEN_MASK_TILT_Y,pressure:(axes[0]*1024.0).round() as u32,tiltX:axes[1].round() as i32,tiltY:axes[2].round() as i32,rotation:axes[3].round() as u32,..Default::default()}}}}else{POINTER_TYPE_INFO {r#type:PT_TOUCH,Anonymous:POINTER_TYPE_INFO_0 {touchInfo:POINTER_TOUCH_INFO {pointerInfo:info,touchMask:TOUCH_MASK_CONTACTAREA|TOUCH_MASK_PRESSURE,rcContact:RECT {left:point.x-2,top:point.y-2,right:point.x+2,bottom:point.y+2},pressure:512,..Default::default()}}}};
        let previous=if pen {self.last_pen_stamp}else{self.last_touch_stamp};
        unsafe {InjectSyntheticPointerInput(device,&[value])}.map_err(|error|format!("InjectSyntheticPointerInput(pen={pen},phase={phase},flags=0x{:x},point=({},{}),pressure={},tilt=({},{}),twist={},qpc={stamp},previous_qpc={previous},gap_us={}): {error}",flags.0,point.x,point.y,axes[0],axes[1],axes[2],axes[3],(stamp-previous) as f64*1_000_000.0/frequency() as f64))?;
        if pen {self.pen_down=phase!=2;self.last_pen=point;self.last_pen_stamp=stamp;}else{self.touch_down=phase!=2;self.last_touch=point;self.last_touch_stamp=stamp;}
        Ok(())
    }
}
impl Drop for Injector {
    fn drop(&mut self) {unsafe {
        if self.mouse_down {let input=INPUT {r#type:INPUT_MOUSE,Anonymous:INPUT_0 {mi:MOUSEINPUT {dwFlags:MOUSEEVENTF_LEFTUP,..Default::default()}}};let _=SendInput(&[input],size_of::<INPUT>() as i32);}
        for device in [self.pen,self.touch].into_iter().flatten() {DestroySyntheticPointerDevice(device);}
        let _=CloseHandle(self.timer);
    }}
}

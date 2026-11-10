use crate::run::{Result,qpc};
use windows::{core::{GUID,HSTRING},Win32::System::Diagnostics::Etw::*};
pub struct Marker(REGHANDLE);
impl Marker {
    pub fn new()->Result<Self>{
        let mut handle=REGHANDLE::default();let provider=GUID::from_u128(0x75de640b_8863_4535_b326_443c2307f3e0);
        let code=unsafe{EventRegister(&provider,None,None,&mut handle)};
        if code!=0{return Err(format!("ETW observer registration failed: {code}").into());}Ok(Self(handle))
    }
    pub fn write(&self,label:&str)->Result<(i64,i64)>{
        let before=qpc();let text=HSTRING::from(format!("CapyMotion {label} qpc={before}"));
        let code=unsafe{EventWriteString(self.0,4,1,&text)};let after=qpc();
        if code!=0{return Err(format!("ETW observer marker failed: {code}").into());}Ok((before,after))
    }
}
impl Drop for Marker {fn drop(&mut self){unsafe{EventUnregister(self.0);}}}

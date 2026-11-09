use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {x:i32,y:i32,width:u32,height:u32}
impl Rect {
    fn parse(text:&str)->Result<Self,String> {
        let fields:Vec<_>=text.split(',').collect();
        if fields.len()!=4{return Err("snapshot rect requires x,y,width,height".into());}
        let value=Self {x:fields[0].parse().map_err(|_|"invalid rect x")?,y:fields[1].parse().map_err(|_|"invalid rect y")?,width:fields[2].parse().map_err(|_|"invalid rect width")?,height:fields[3].parse().map_err(|_|"invalid rect height")?};
        if value.x<0||value.y<0||value.width==0||value.height==0||value.x as i64+value.width as i64>i32::MAX as i64||value.y as i64+value.height as i64>i32::MAX as i64{return Err("snapshot rect requires nonnegative coordinates and positive dimensions within i32 bounds".into());}
        Ok(value)
    }
    fn fits(self,width:u32,height:u32)->bool {self.x>=0&&self.y>=0&&self.x as u64+self.width as u64<=width as u64&&self.y as u64+self.height as u64<=height as u64}
}
#[derive(Debug)]
struct Options {hwnd:usize,pid:u32,out:PathBuf,rect:Option<Rect>,screen:bool}
impl Options {
    fn parse(args:impl IntoIterator<Item=String>)->Result<Self,String> {
        let mut args=args.into_iter();let (mut hwnd,mut pid,mut out,mut rect)=(None,None,None,None);let mut screen=false;let mut seen=std::collections::BTreeSet::new();
        while let Some(arg)=args.next(){
            if !seen.insert(arg.clone()){return Err(format!("duplicate snapshot option: {arg}"));}
            if arg=="--screen"{screen=true;continue;}
            let value=args.next().ok_or_else(||format!("missing value for {arg}"))?;
            match arg.as_str(){
                "--hwnd"=>hwnd=Some(value.parse::<usize>().map_err(|_|"hwnd must be a decimal integer")?),
                "--pid"=>pid=Some(value.parse::<u32>().map_err(|_|"pid must be a decimal integer")?),
                "--out"=>{if value.contains('\0'){return Err("output path contains a null character".into());}out=Some(PathBuf::from(value));},
                "--rect"=>rect=Some(Rect::parse(&value)?),
                _=>return Err(format!("unknown snapshot option: {arg}")),
            }
        }
        let value=Self {hwnd:hwnd.ok_or("snapshot requires --hwnd")?,pid:pid.ok_or("snapshot requires --pid")?,out:out.ok_or("snapshot requires --out")?,rect,screen};
        if value.hwnd==0||value.pid==0{return Err("snapshot requires nonzero hwnd and pid".into());}
        if !value.out.is_absolute()||!value.out.extension().is_some_and(|ext|ext.eq_ignore_ascii_case("png")){return Err("snapshot --out must be an absolute PNG path".into());}
        if value.screen&&value.rect.is_some(){return Err("--screen and --rect are mutually exclusive".into());}
        Ok(value)
    }
}
fn private_desktop(value:Option<&str>)->Result<&str,String> {
    value.filter(|name|!name.trim().is_empty()&&!name.eq_ignore_ascii_case("Default")).ok_or_else(||"CAPY_PRIVATE_DESKTOP must name the owned private test desktop".into())
}
fn owner(expected:u32,actual:u32,live:bool)->Result<(),String> {
    if expected==0||actual!=expected{return Err("snapshot window does not belong to the recorded PID".into());}
    if !live{return Err("snapshot process is no longer running".into());}
    Ok(())
}

fn bgra(pixels:&[[u8;4]])->Vec<u8> {
    pixels.iter().flat_map(|p|[p[2],p[1],p[0],p[3]]).collect()
}

#[cfg(windows)]
pub fn run()->crate::run::Result<()> {
    use crate::run::{Result,qpc};
    use windows::{core::{Interface,PCWSTR},Win32::{Foundation::*,Graphics::{Direct3D11::ID3D11Texture2D,Dxgi::*,Imaging::*},System::{Com::*,StationsAndDesktops::*,Threading::*},UI::{HiDpi::*,WindowsAndMessaging::*}}};
    struct Com;
    impl Drop for Com {fn drop(&mut self){unsafe{CoUninitialize();}}}
    struct InputDesktop(HDESK);
    impl Drop for InputDesktop {fn drop(&mut self){let _=unsafe{CloseDesktop(self.0)};}}
    struct Process(HANDLE);
    impl Drop for Process {fn drop(&mut self){let _=unsafe{CloseHandle(self.0)};}}
    struct Frame<'a> {dup:&'a IDXGIOutputDuplication,released:bool}
    impl Frame<'_> {
        fn release(mut self)->Result<()> {self.released=true;unsafe{self.dup.ReleaseFrame()?;}Ok(())}
    }
    impl Drop for Frame<'_> {fn drop(&mut self){if !self.released{let _=unsafe{self.dup.ReleaseFrame()};}}}
    fn desktop_name(desktop:HDESK)->Result<String> {
        let mut needed=0;let handle=HANDLE(desktop.0);
        let _=unsafe{GetUserObjectInformationW(handle,UOI_NAME,None,0,Some(&mut needed))};
        if !(2..=65536).contains(&needed){return Err("desktop name size is unavailable".into());}
        let mut name=vec![0u16;needed.div_ceil(2) as usize];
        unsafe{GetUserObjectInformationW(handle,UOI_NAME,Some(name.as_mut_ptr().cast()),needed,Some(&mut needed))?;}
        let length=name.iter().position(|c|*c==0).unwrap_or(name.len());Ok(String::from_utf16(&name[..length])?)
    }
    let options=Options::parse(std::env::args().skip(2))?;
    let private=std::env::var("CAPY_PRIVATE_DESKTOP").ok();let expected=private_desktop(private.as_deref())?;
    unsafe{SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)?;CoInitializeEx(None,COINIT_MULTITHREADED).ok()?;}
    let _com=Com;
    let hwnd=HWND(options.hwnd as _);let mut actual=0;
    let window_thread=unsafe{GetWindowThreadProcessId(hwnd,Some(&mut actual))};
    if window_thread==0||!unsafe{IsWindow(Some(hwnd))}.as_bool(){return Err("snapshot HWND is no longer valid".into());}
    owner(options.pid,actual,true)?;
    let process=Process(unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,false,options.pid)?});
    owner(options.pid,actual,unsafe{WaitForSingleObject(process.0,0)}==WAIT_TIMEOUT)?;
    let input=InputDesktop(unsafe{OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0),false,DESKTOP_READOBJECTS)?});
    let current=desktop_name(unsafe{GetThreadDesktop(GetCurrentThreadId())?})?;
    let target=desktop_name(unsafe{GetThreadDesktop(window_thread)?})?;
    if current!=expected||target!=expected||desktop_name(input.0)?!=expected{return Err("snapshot thread, input and owned window must be on CAPY_PRIVATE_DESKTOP".into());}
    let desktop=crate::capture::Desktop::new()?;let display=desktop.desc.DesktopCoordinates;
    let (client,client_origin,_)=crate::launch::bounds(hwnd)?;
    let (roi,origin)=if options.screen {
        (Rect{x:0,y:0,width:(display.right-display.left) as u32,height:(display.bottom-display.top) as u32},POINT{x:display.left,y:display.top})
    }else{
        let width=client.right.max(0) as u32;let height=client.bottom.max(0) as u32;
        let roi=options.rect.unwrap_or(Rect{x:0,y:0,width,height});
        if width==0||height==0||!roi.fits(width,height){return Err("snapshot rectangle lies outside the owned client".into());}
        (roi,POINT{x:client_origin.x.checked_add(roi.x).ok_or("snapshot x overflow")?,y:client_origin.y.checked_add(roi.y).ok_or("snapshot y overflow")?})
    };
    let region=crate::capture::Region::new(&desktop._device,roi.width,roi.height)?;
    let mut info=DXGI_OUTDUPL_FRAME_INFO::default();let mut resource=None;
    unsafe{desktop.dup.AcquireNextFrame(5000,&mut info,&mut resource)?;}
    let frame=Frame{dup:&desktop.dup,released:false};let at=qpc();
    let source:ID3D11Texture2D=resource.ok_or("snapshot frame has no desktop resource")?.cast()?;
    if !region.copy(&desktop.context,&source,origin.x-display.left,origin.y-display.top,[(display.right-display.left) as u32,(display.bottom-display.top) as u32]){return Err("snapshot rectangle lies outside the captured primary output".into());}
    let pixels=region.map(&desktop.context)?;frame.release()?;
    let bgra=bgra(&pixels.rgba);
    let stride=roi.width.checked_mul(4).ok_or("snapshot row stride overflow")?;
    if bgra.len()>u32::MAX as usize{return Err("snapshot PNG buffer exceeds WIC limits".into());}
    let factory:IWICImagingFactory=unsafe{CoCreateInstance(&CLSID_WICImagingFactory,None,CLSCTX_INPROC_SERVER)?};
    let stream=unsafe{factory.CreateStream()?};
    use std::os::windows::ffi::OsStrExt;
    let filename:Vec<_>=options.out.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe{stream.InitializeFromFilename(PCWSTR(filename.as_ptr()),GENERIC_WRITE.0)?;}
    let encoder=unsafe{factory.CreateEncoder(&GUID_ContainerFormatPng,std::ptr::null())?};
    unsafe{encoder.Initialize(&stream,WICBitmapEncoderNoCache)?;}
    let (mut encoded,mut properties)=(None,None);
    unsafe{encoder.CreateNewFrame(&mut encoded,&mut properties)?;}
    let encoded=encoded.ok_or("WIC PNG encoder returned no frame")?;let mut format=GUID_WICPixelFormat32bppBGRA;
    unsafe{encoded.Initialize(properties.as_ref())?;encoded.SetSize(roi.width,roi.height)?;encoded.SetPixelFormat(&mut format)?;}
    if format!=GUID_WICPixelFormat32bppBGRA{return Err("WIC PNG encoder did not accept BGRA pixels".into());}
    unsafe{encoded.WritePixels(roi.height,stride,&bgra)?;encoded.Commit()?;encoder.Commit()?;}
    println!("{}",serde_json::json!({"out":options.out,"width":roi.width,"height":roi.height,"origin":[origin.x,origin.y],"qpc":at,"last_present_qpc":info.LastPresentTime,"hwnd":options.hwnd,"pid":options.pid}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(extra:&[&str])->Vec<String> {
        let out=if cfg!(windows){r"C:\capture\out.png"}else{"/capture/out.png"};
        ["--hwnd","123","--pid","456","--out",out].into_iter().chain(extra.iter().copied()).map(str::to_owned).collect()
    }
    #[test]
    fn snapshot_rect_and_recorded_ownership_are_explicit() {
        let options=Options::parse(args(&["--rect","1,2,300,200"])).unwrap();assert_eq!(options.hwnd,123);assert_eq!(options.pid,456);assert_eq!(options.rect,Some(Rect{x:1,y:2,width:300,height:200}));assert!(!options.screen);assert!(options.out.is_absolute());assert!(Options::parse(args(&["--screen"])).unwrap().screen);
        assert!(owner(456,456,true).is_ok());assert!(owner(456,123,true).is_err());assert!(owner(456,456,false).is_err());
    }
    #[test]
    fn malformed_or_unbounded_snapshot_requests_are_rejected() {
        for text in ["1,2,3","-1,2,3,4","0,0,0,1","0,0,1,-1","2147483647,0,1,1"]{assert!(Rect::parse(text).is_err());}
        assert!(!Rect::parse("1,2,300,200").unwrap().fits(300,200));assert!(Rect::parse("1,2,300,200").unwrap().fits(301,202));
        assert!(Options::parse(args(&["--screen","--rect","0,0,1,1"])).is_err());
        assert!(Options::parse(args(&["--pid","456"])).is_err());
        assert!(Options::parse(["--hwnd","0","--pid","456","--out","relative.png"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--hwnd","123"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--hwnd","123","--pid","456","--out","relative.png"].into_iter().map(str::to_owned)).is_err());
    }
    #[test]
    fn png_bgra_preserves_red_blue_and_alpha_channels() {
        assert_eq!(bgra(&[[255,0,0,255],[0,0,255,128]]),vec![0,0,255,255,255,0,0,128]);
    }
    #[test]
    fn snapshots_require_the_named_private_desktop() {
        assert!(private_desktop(None).is_err());assert!(private_desktop(Some("")).is_err());assert!(private_desktop(Some("Default")).is_err());assert!(private_desktop(Some("capy-private-123")).is_ok());
    }
}

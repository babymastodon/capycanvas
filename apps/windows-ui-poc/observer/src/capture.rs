use crate::{launch,run::{Result,qpc,write_line}};
use poc_observer::decode::{Pixels,CaptureStats};
use std::{fs::File,io::BufWriter,sync::{Arc,atomic::{AtomicBool,AtomicI32,AtomicU32,AtomicUsize,Ordering},mpsc::Sender}};
use windows::{core::Interface,Win32::{Foundation::*,Graphics::{Direct3D::*,Direct3D11::*,Dxgi::*,Dxgi::Common::*,Gdi::*}}};

pub(crate) struct Region { texture:ID3D11Texture2D, width:u32,height:u32 }
impl Region {
    pub(crate) fn new(device:&ID3D11Device,width:u32,height:u32)->Result<Self> {
        let desc=D3D11_TEXTURE2D_DESC { Width:width,Height:height,MipLevels:1,ArraySize:1,Format:DXGI_FORMAT_B8G8R8A8_UNORM,SampleDesc:DXGI_SAMPLE_DESC {Count:1,Quality:0},Usage:D3D11_USAGE_STAGING,CPUAccessFlags:D3D11_CPU_ACCESS_READ.0 as u32,..Default::default() };
        let mut texture=None;
        unsafe { device.CreateTexture2D(&desc,None,Some(&mut texture))?; }
        Ok(Self {texture:texture.ok_or("D3D11 returned no staging texture")?,width,height})
    }
    pub(crate) fn copy(&self,context:&ID3D11DeviceContext,source:&ID3D11Texture2D,x:i32,y:i32,desktop:[u32;2])->bool {
        if x<0 || y<0 || x as u32+self.width>desktop[0] || y as u32+self.height>desktop[1] { return false; }
        let region=D3D11_BOX {left:x as u32,top:y as u32,front:0,right:x as u32+self.width,bottom:y as u32+self.height,back:1};
        unsafe {context.CopySubresourceRegion(&self.texture,0,0,0,0,source,0,Some(&region));}
        true
    }
    pub(crate) fn map(&self,context:&ID3D11DeviceContext)->Result<Pixels> {
        let mut mapped=D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {context.Map(&self.texture,0,D3D11_MAP_READ,0,Some(&mut mapped))?;}
        let mut rgba=Vec::with_capacity((self.width*self.height) as usize);
        for yy in 0..self.height {
            let bytes=unsafe {std::slice::from_raw_parts((mapped.pData as *const u8).add((yy*mapped.RowPitch) as usize),(self.width*4) as usize)};
            for pixel in bytes.as_chunks::<4>().0 {rgba.push([pixel[2],pixel[1],pixel[0],pixel[3]]);}
        }
        unsafe {context.Unmap(&self.texture,0);}
        Ok(Pixels {width:self.width as usize,height:self.height as usize,rgba})
    }
}
struct FrameRegions { width:u32,height:u32,magenta:Region,cyan:Region,orange:Region,drawer:Region,ruler:Region,widthline:Region,slider:Region }
pub(crate) struct Desktop { pub(crate) _device:ID3D11Device,pub(crate) context:ID3D11DeviceContext,pub(crate) dup:IDXGIOutputDuplication,pub(crate) desc:DXGI_OUTPUT_DESC, info:serde_json::Value }
impl Desktop {
    pub(crate) fn new()->Result<Self> {
        let monitor=unsafe {MonitorFromPoint(POINT {x:0,y:0},MONITOR_DEFAULTTOPRIMARY)};
        let factory:IDXGIFactory1=unsafe {CreateDXGIFactory1()?};
        let mut ai=0;
        while let Ok(adapter)=unsafe {factory.EnumAdapters1(ai)} {
            let mut oi=0;
            while let Ok(output)=unsafe {adapter.EnumOutputs(oi)} {
                let desc=unsafe {output.GetDesc()?};
                if desc.Monitor==monitor {
                    let (mut device,mut context)=(None,None);
                    unsafe {D3D11CreateDevice(&adapter,D3D_DRIVER_TYPE_UNKNOWN,HMODULE::default(),D3D11_CREATE_DEVICE_BGRA_SUPPORT,None,D3D11_SDK_VERSION,Some(&mut device),None,Some(&mut context))?;}
                    let device=device.ok_or("D3D11 returned no device")?;
                    let dup=unsafe {output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?};
                    let ad=unsafe {adapter.GetDesc1()?};
                    let name=String::from_utf16_lossy(&ad.Description).trim_end_matches('\0').to_string();
                    let info=serde_json::json!({"adapter":name,"adapter_luid":{"low":ad.AdapterLuid.LowPart,"high":ad.AdapterLuid.HighPart},"monitor":desc.Monitor.0 as usize,"desktop_rect":[desc.DesktopCoordinates.left,desc.DesktopCoordinates.top,desc.DesktopCoordinates.right,desc.DesktopCoordinates.bottom],"rotation":desc.Rotation.0});
                    if desc.Rotation!=DXGI_MODE_ROTATION_IDENTITY {return Err("rotated desktop capture is unsupported by this POC".into());}
                    return Ok(Self {_device:device,context:context.ok_or("D3D11 returned no context")?,dup,desc,info});
                }
                oi+=1;
            }
            ai+=1;
        }
        Err("no DXGI output matches the primary monitor".into())
    }
}

pub fn run(path:&std::path::Path,hwnd:Arc<AtomicUsize>,stop:Arc<AtomicBool>,ready:Sender<std::result::Result<serde_json::Value,String>>,slider_y:Arc<AtomicI32>,pen_seen:Arc<AtomicU32>)->Result<CaptureStats> {
    let mut cpu=poc_observer::cpu::ThreadCpu::new("capture",&path.with_file_name("observer-cpu.jsonl"),Some(windows::Win32::System::Threading::THREAD_PRIORITY_HIGHEST))?;
    let desktop=match Desktop::new() {Ok(d)=>d,Err(e)=>{let _=ready.send(Err(e.to_string()));return Err(e);}};
    ready.send(Ok(desktop.info.clone())).map_err(|_|"capture readiness receiver closed")?;
    let mut writer=BufWriter::new(File::create(path)?);
    let mut stats=CaptureStats {capture_start_qpc:qpc(),..Default::default()};
    let mut regions:Option<FrameRegions>=None;
    let sample=Region::new(&desktop._device,1,1)?;
    let ui_marker=Region::new(&desktop._device,98,6)?;
    let pen_marker=Region::new(&desktop._device,98,6)?;
    let grid=Region::new(&desktop._device,16,16)?;
    let out=desktop.desc.DesktopCoordinates;
    let extent=[(out.right-out.left) as u32,(out.bottom-out.top) as u32];
    while !stop.load(Ordering::Acquire) {
        cpu.sample()?;
        let mut info=DXGI_OUTDUPL_FRAME_INFO::default(); let mut resource=None;
        match unsafe {desktop.dup.AcquireNextFrame(100,&mut info,&mut resource)} {
            Err(e) if e.code()==DXGI_ERROR_WAIT_TIMEOUT=>continue,
            Err(e)=>{stats.capture_failure=Some(e.to_string());break;},
            Ok(())=>{}
        }
        let result=(||->Result<()> {
            let at=qpc();
            if info.LastPresentTime==0 {stats.cursor_only_frames+=1;}else{stats.desktop_frames+=1;}
            if info.AccumulatedFrames>1 {stats.coalesced_frames+=1;stats.missing_desktop_frames+=(info.AccumulatedFrames-1) as u64;}
            let raw=hwnd.load(Ordering::Acquire);
            if raw==0 { return Ok(()); }
            let window=HWND(raw as _);
            let (rect,origin,scale)=launch::bounds(window)?;
            let w=rect.right.max(0) as u32;let h=rect.bottom.max(0) as u32;
            if w<100||h<100 {return Ok(());}
            if regions.as_ref().is_none_or(|r|r.width!=w||r.height!=h) {
                regions=Some(FrameRegions {width:w,height:h,magenta:Region::new(&desktop._device,1,h)?,cyan:Region::new(&desktop._device,1,h)?,orange:Region::new(&desktop._device,1,h)?,drawer:Region::new(&desktop._device,w.min(extent[0]),1)?,ruler:Region::new(&desktop._device,1,h)?,widthline:Region::new(&desktop._device,w.min(extent[0]),1)?,slider:Region::new(&desktop._device,w.min(extent[0]),1)?});
            }
            let FrameRegions {magenta,cyan,orange,drawer:horizontal,ruler,widthline,slider,..}=regions.as_ref().unwrap();
            let source:ID3D11Texture2D=resource.as_ref().ok_or("desktop frame has no resource")?.cast()?;
            let x=origin.x-out.left;let y=origin.y-out.top;
            let col=w as f64-320.0*scale;let colw=304.0*scale;
            let ui_valid=ui_marker.copy(&desktop.context,&source,x+3,y+43,extent);
            let pen_valid=pen_marker.copy(&desktop.context,&source,x+3,y+h as i32-13,extent);
            let magenta_valid=magenta.copy(&desktop.context,&source,x+(col+colw/6.0).round() as i32,y,extent);
            let cyan_valid=cyan.copy(&desktop.context,&source,x+(col+colw*0.5).round() as i32,y,extent);
            let orange_valid=orange.copy(&desktop.context,&source,x+(col+colw*5.0/6.0).round() as i32,y,extent);
            let drawer_valid=horizontal.copy(&desktop.context,&source,x,y+(300.0*scale).round() as i32,extent);
            let width_valid=widthline.copy(&desktop.context,&source,x,y+(20.0*scale).round() as i32,extent);
            let zen_valid=sample.copy(&desktop.context,&source,x+202,y+18,extent);
            let ruler_valid=ruler.copy(&desktop.context,&source,x+col.round() as i32+2,y,extent);
            let slider_row=slider_y.load(Ordering::Acquire);
            let slider_valid=slider_row>=0 && slider_row<h as i32 && slider.copy(&desktop.context,&source,x,y+slider_row,extent);
            let mut grid_valid=true;
            for gy in 0..16 {for gx in 0..16 {
                let px=x+((gx+1)*w/17) as i32;let py=y+((gy+1)*h/17) as i32;
                if px<0||py<0||px>=extent[0] as i32||py>=extent[1] as i32 {grid_valid=false;continue;}
                let area=D3D11_BOX {left:px as u32,top:py as u32,front:0,right:px as u32+1,bottom:py as u32+1,back:1};
                unsafe {desktop.context.CopySubresourceRegion(&grid.texture,0,gx,gy,0,&source,0,Some(&area));}
            }}
            let read=|region:&Region,valid:bool|->Result<Option<Pixels>> {if valid {Ok(Some(region.map(&desktop.context)?))}else{Ok(None)}};
            let ui=read(&ui_marker,ui_valid)?.and_then(|p|p.marker().ok());
            let pen=read(&pen_marker,pen_valid)?.and_then(|p|p.marker().ok());
            if ui.is_some(){stats.valid_ui_markers+=1;}else{stats.marker_errors+=1;}
            if let Some(pen)=pen {stats.valid_canvas_markers+=1;stats.max_pen_samples=stats.max_pen_samples.max(pen);pen_seen.fetch_max(pen,Ordering::Release);}else{stats.marker_errors+=1;}
            let divider=read(magenta,magenta_valid)?.and_then(|p|p.color_run([255,0,255],true)).map(|run|run.end);
            let glass=read(cyan,cyan_valid)?.and_then(|p|p.color_run([0,255,255],true)).map(|run|run.start);
            let fill=read(orange,orange_valid)?.and_then(|p|p.color_run([255,128,0],true)).map(|run|run.end);
            let drawer=read(horizontal,drawer_valid)?.and_then(|p|p.color_run([255,255,0],false)).map(|run|run.start);
            let width=read(widthline,width_valid)?.and_then(|p|p.color_run([0,0,255],false)).map(|run|run.end);
            let slider_x=read(slider,slider_valid)?.and_then(|p|p.color_run([0,255,0],false)).map(|run|run.end);
            let zen=read(&sample,zen_valid)?.and_then(|p|p.at(0,0)).map(|p|(p[0] as f64+p[1] as f64+p[2] as f64)/(3.0*255.0));
            let list_top=(92.0*scale).round() as i32;
            let scroll=read(ruler,ruler_valid)?.and_then(|mut p| {
                let start=list_top.max(0) as usize;let end=divider.unwrap_or((h as f64*0.6) as i32).max(0) as usize;
                if start>=end || end>p.height {return None;}
                p.rgba=p.rgba[start..end].to_vec();p.height=end-start;p.ruler(44.0*scale,list_top)
            });
            let black=if grid_valid {Some(grid.map(&desktop.context)?.rgba.iter().filter(|p|p[..3]==[0,0,0]).count())}else{None};
            write_line(&mut writer,&serde_json::json!({"qpc":at,"last_present_qpc":info.LastPresentTime,"cursor_only":info.LastPresentTime==0,"accumulated_frames":info.AccumulatedFrames,"ui_marker":ui,"ui_frame":ui.map(|v|v&255),"state16":ui.map(|v|v>>8),"pen_samples":pen,"divider_y":divider,"glass_y":glass,"fill_y":fill,"drawer_x":drawer,"zen_opacity":zen,"scroll_offset":scroll,"slider_x":slider_x,"width":width,"black_samples":black,"marker_errors":u32::from(ui.is_none())+u32::from(pen.is_none()),"capture_ms":(qpc()-at) as f64*1000.0/crate::run::frequency() as f64}))?;
            Ok(())
        })();
        unsafe {desktop.dup.ReleaseFrame()?;}
        if let Err(e)=result {stats.capture_failure=Some(e.to_string());break;}
    }
    stats.capture_end_qpc=qpc();
    cpu.finish()?;
    use std::io::Write;writer.flush()?;
    Ok(stats)
}

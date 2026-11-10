use crate::{run::{Result,qpc},probes::Pixels};
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
    fn grid(&self,context:&ID3D11DeviceContext,source:&ID3D11Texture2D,rect:[i32;4],desktop:[u32;2])->bool {
        if rect[0]<0||rect[1]<0||rect[0]+rect[2]>desktop[0] as i32||rect[1]+rect[3]>desktop[1] as i32{return false;}
        for y in 0..16{for x in 0..16{let px=(rect[0]+(x+1)*rect[2]/17) as u32;let py=(rect[1]+(y+1)*rect[3]/17) as u32;
            let area=D3D11_BOX{left:px,top:py,front:0,right:px+1,bottom:py+1,back:1};unsafe{context.CopySubresourceRegion(&self.texture,0,x as u32,y as u32,0,source,0,Some(&area));}
        }}true
    }
}
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

use std::{fs::File,io::{BufWriter,Write},path::Path,sync::{Arc,atomic::{AtomicBool,Ordering},mpsc::Sender}};
use crate::probes::{Probe,Spec};
struct Slot {regions:Vec<Region>,stamp:i64,copied:i64,copy_ms:f64,accumulated:u32}
struct Captured {stamp:i64,copied:i64,mapped:i64,capture_ms:f64,accumulated:u32,images:Vec<Pixels>}
pub fn run(path:&Path,specs:Vec<Spec>,stop:Arc<AtomicBool>,ready:Sender<std::result::Result<serde_json::Value,String>>)->Result<()> {
    let mut cpu=crate::cpu::ThreadCpu::new("capture",&path.with_file_name("observer-cpu.jsonl"),Some(windows::Win32::System::Threading::THREAD_PRIORITY_HIGHEST))?;
    let desktop=match Desktop::new(){Ok(d)=>d,Err(error)=>{let _=ready.send(Err(error.to_string()));return Err(error);}};
    let extent=[(desktop.desc.DesktopCoordinates.right-desktop.desc.DesktopCoordinates.left) as u32,(desktop.desc.DesktopCoordinates.bottom-desktop.desc.DesktopCoordinates.top) as u32];
    let mut slots=(0..3).map(|_|Ok(Slot {regions:specs.iter().map(|s|if s.kind=="black-grid"{Region::new(&desktop._device,16,16)}else{Region::new(&desktop._device,s.rect[2] as u32,s.rect[3] as u32)}).collect::<Result<Vec<_>>>()?,stamp:0,copied:0,copy_ms:0.,accumulated:0})).collect::<Result<Vec<_>>>()?;
    let mut records=Vec::new();let mut next=0usize;let mut sent=false;
    let drain=|slot:&mut Slot,records:&mut Vec<Captured>|->Result<()> {
        if slot.stamp==0{return Ok(());}
        let began=qpc();let images=slot.regions.iter().map(|region|region.map(&desktop.context)).collect::<Result<Vec<_>>>()?;
        let mapped=qpc();records.push(Captured {stamp:slot.stamp,accumulated:slot.accumulated,copied:slot.copied,mapped,capture_ms:slot.copy_ms+(mapped-began) as f64*1000./crate::run::frequency() as f64,images});slot.stamp=0;Ok(())
    };
    while !stop.load(Ordering::Acquire){
        cpu.sample()?;let mut info=DXGI_OUTDUPL_FRAME_INFO::default();let mut resource=None;
        match unsafe{desktop.dup.AcquireNextFrame(20,&mut info,&mut resource)}{
            Err(error) if error.code()==DXGI_ERROR_WAIT_TIMEOUT=>{for slot in &mut slots{drain(slot,&mut records)?;}continue;},Err(error)=>return Err(error.into()),Ok(())=>{}
        }
        let result=(||->Result<()>{
            if info.LastPresentTime==0{return Ok(());}
            let previous=(next+2)%3;drain(&mut slots[previous],&mut records)?;
            let source:ID3D11Texture2D=resource.as_ref().ok_or("DD frame has no resource")?.cast()?;
            let slot=&mut slots[next];let began=qpc();
            for (spec,region) in specs.iter().zip(&slot.regions){let rect=[spec.rect[0]-desktop.desc.DesktopCoordinates.left,spec.rect[1]-desktop.desc.DesktopCoordinates.top,spec.rect[2],spec.rect[3]];let copied=if spec.kind=="black-grid"{region.grid(&desktop.context,&source,rect,extent)}else{region.copy(&desktop.context,&source,rect[0],rect[1],extent)};if !copied{return Err(format!("capture ROI outside primary output: {}",spec.name).into());}}
            slot.stamp=info.LastPresentTime;slot.copied=qpc();slot.accumulated=info.AccumulatedFrames;unsafe{desktop.context.Flush();}slot.copy_ms=(qpc()-began) as f64*1000./crate::run::frequency() as f64;next=(next+1)%3;
            if !sent{ready.send(Ok(desktop.info.clone())).map_err(|_|"capture readiness receiver closed")?;sent=true;}Ok(())
        })();unsafe{desktop.dup.ReleaseFrame()?;}result?;
    }
    for slot in &mut slots{drain(slot,&mut records)?;}cpu.finish()?;
    records.sort_by_key(|r|r.stamp);let mut probes:Vec<_>=specs.iter().cloned().map(Probe::new).collect();let mut offset=0usize;
    let mut output=BufWriter::new(File::create(path)?);let mut pixels=BufWriter::new(File::create(path.with_file_name("pixels.rgba"))?);
    for record in records{
        let mut observations=serde_json::Map::new();
        for ((spec,image),probe) in specs.iter().zip(record.images).zip(probes.iter_mut()){
            let shape=[image.height,image.width];pixels.write_all(image.rgba.as_flattened())?;let next=offset+image.rgba.len()*4;
            let mut observation=serde_json::to_value(probe.observe(image))?;observation["pixel_offset"]=offset.into();observation["pixel_shape"]=serde_json::json!(shape);observations.insert(spec.name.clone(),observation);offset=next;
        }
        serde_json::to_writer(&mut output,&serde_json::json!({"last_present_qpc":record.stamp,"accumulated_frames":record.accumulated,"copied_qpc":record.copied,"mapped_qpc":record.mapped,"capture_ms":record.capture_ms,"probes":observations}))?;output.write_all(b"\n")?;
    }
    output.flush()?;pixels.flush()?;Ok(())
}
